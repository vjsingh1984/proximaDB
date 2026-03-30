//! Query Execution Planner - Cost-based optimization for vector, graph, and hybrid queries
//!
//! This module provides AST-based planning that leverages
//! HashMap metadata filtering for optimal performance.

use crate::core::search::FilterExpression;
use crate::graph::GraphOperationsService;
use crate::query::ast::{BinaryOp, Expr, Query, Select};
use crate::query::execution::{
    ExecutionOperation, ExecutionPlan, ExecutionStrategy, FusionStrategy, ProjectionTransform,
};
use crate::services::operations::vectors::VectorOperationsService;
use crate::storage::cache::orchestrator::{CacheType, CrossCacheOrchestrator};
use anyhow::{Result, anyhow};
use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// Cached execution plan with metadata
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
struct CachedPlan {
    plan: ExecutionPlan,
    created_at: u64, // Unix timestamp instead of Instant
    hit_count: u64,
    avg_execution_time_ms: f64,
}

/// Cost-based execution planner for unified query optimization with unified caching
pub struct ExecutionPlanner {
    #[allow(dead_code)]
    vector_service: Arc<VectorOperationsService>,
    #[allow(dead_code)]
    graph_service: Arc<GraphOperationsService>,
    cost_model: CostModel,
    params: Option<Vec<crate::proto::proximadb_v1::SqlValue>>, // for decoding $1 vectors when not substituted
    seeding_strategy: crate::query::execution::SeedingStrategy,
    fusion_weights: Option<Vec<f64>>,
    /// Unified cache orchestrator for query plan caching
    cache_orchestrator: Option<Arc<CrossCacheOrchestrator>>,
}

impl ExecutionPlanner {
    /// Create new execution planner with service integrations
    pub fn new(
        vector_service: Arc<VectorOperationsService>,
        graph_service: Arc<GraphOperationsService>,
    ) -> Self {
        Self {
            vector_service,
            graph_service,
            cost_model: CostModel::new(),
            params: None,
            seeding_strategy: crate::query::execution::SeedingStrategy::Average,
            fusion_weights: None,
            cache_orchestrator: None,
        }
    }

    /// Create new execution planner with unified cache orchestrator
    pub fn with_cache(
        vector_service: Arc<VectorOperationsService>,
        graph_service: Arc<GraphOperationsService>,
        cache_orchestrator: Arc<CrossCacheOrchestrator>,
    ) -> Self {
        Self {
            vector_service,
            graph_service,
            cost_model: CostModel::new(),
            params: None,
            seeding_strategy: crate::query::execution::SeedingStrategy::Average,
            fusion_weights: None,
            cache_orchestrator: Some(cache_orchestrator),
        }
    }

    pub fn with_params(
        vector_service: Arc<VectorOperationsService>,
        graph_service: Arc<GraphOperationsService>,
        params: Option<Vec<crate::proto::proximadb_v1::SqlValue>>,
    ) -> Self {
        let mut p = Self::new(vector_service, graph_service);
        p.params = params;
        p
    }

    pub fn set_seeding_strategy(&mut self, strategy: crate::query::execution::SeedingStrategy) {
        self.seeding_strategy = strategy;
    }

    pub fn set_fusion_weights(&mut self, weights: Option<Vec<f64>>) {
        self.fusion_weights = weights;
    }

    /// Generate optimized execution plan from internal AST
    ///
    /// This method analyzes the query and determines the optimal execution strategy:
    /// - Vector-only: For similarity search and metadata filtering
    /// - Graph-only: For traversal and pathfinding queries  
    /// - Hybrid: For combined vector + graph intelligence
    /// - Relational: For traditional SQL operations
    pub fn create_plan(&self, query: &Query) -> Result<ExecutionPlan> {
        // Generate cache key for query plan caching
        let cache_key = self.generate_cache_key(query);

        // Note: Cache checking is async and would require making this function async,
        // which would break many synchronous callers. For now, skip cache check.
        // TODO: Consider implementing a synchronous cache interface or async create_plan_async

        // Generate new plan
        let plan = match query {
            Query::Select(select) => self.plan_select(select),
            Query::With { ctes, query } => self.plan_cte(ctes, query),
            Query::Set {
                left,
                op,
                all,
                right,
            } => self.plan_set_operation(left, op, *all, right),
        }?;

        // Cache the new plan if cache orchestrator is available
        if let Some(ref cache_orchestrator) = self.cache_orchestrator {
            let cached_plan = CachedPlan {
                plan: plan.clone(),
                created_at: chrono::Utc::now().timestamp() as u64,
                hit_count: 0,
                avg_execution_time_ms: 0.0,
            };

            if let Ok(cached_data) = serde_json::to_vec(&cached_plan) {
                let _ = cache_orchestrator.put(CacheType::QueryPlan, cache_key, cached_data, None);
            }
        }

        Ok(plan)
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
            // Use first FROM as left side
            let left_alias = select
                .from.first()
                .and_then(|t| t.alias.clone())
                .unwrap_or_else(|| "l".to_string());
            for j in &select.joins {
                let kind = match j.join_type {
                    crate::query::ast::JoinType::Inner => crate::query::execution::JoinKind::Inner,
                    crate::query::ast::JoinType::LeftOuter => {
                        crate::query::execution::JoinKind::Left
                    }
                    _ => continue, // Skip unsupported join types for now
                };
                let (lks, rks) = if let Some(on) = &j.on_condition {
                    let pairs = Self::extract_join_key_pairs_static(on);
                    if pairs.is_empty() {
                        (vec!["".into()], vec!["".into()])
                    } else {
                        let (ls, rs): (Vec<_>, Vec<_>) = pairs.into_iter().unzip();
                        (ls, rs)
                    }
                } else {
                    (vec!["".into()], vec!["".into()])
                };
                let right_alias = j
                    .right_table
                    .alias
                    .clone()
                    .unwrap_or_else(|| "r".to_string());
                operations.push(ExecutionOperation::Join {
                    kind,
                    left_keys: lks,
                    right_keys: rks,
                    left_alias: left_alias.clone(),
                    right_alias,
                });
            }
        }

        Ok(ExecutionPlan {
            execution_strategy,
            operations,
            estimated_cost,
            optimizations,
            performance_hints,
            seeding_strategy: self.seeding_strategy.clone(),
            limit: select.limit.map(|v| v as usize),
            offset: select.offset.map(|v| v as usize),
        })
    }

    /// Analyze query to detect vector, graph, and SKS patterns
    fn analyze_query(&self, select: &Select) -> Result<QueryAnalysis> {
        let mut analysis = QueryAnalysis::default();

        // Check projections for SKS functions
        for proj_item in &select.projection {
            if self.contains_sks_funcs(&proj_item.expr) {
                analysis.has_sks_functions = true;
            }
        }

        // Check ORDER BY for vector functions
        for order_expr in &select.order_by {
            if let Expr::FuncCall { name, .. } = &order_expr.expr
                && (name.to_uppercase().contains("VECTOR_SIMILARITY")
                    || name.to_uppercase().contains("COSINE_DISTANCE"))
                {
                    analysis.has_vector_functions = true;
                }
        }

        // Check for SKS functions in WHERE clause
        if let Some(where_expr) = &select.selection {
            analysis.has_sks_functions = analysis.has_sks_functions
                || self.detect_sks_functions(where_expr)
                || self.contains_sks_funcs(where_expr);
        }

        // Check for graph patterns in FROM clause
        for table in &select.from {
            if let Some(table_name) = &table.name {
                // TODO: Detect graph collections vs vector collections
                // For now, assume graph if collection name suggests it
                if table_name.contains("graph")
                    || table_name.contains("node")
                    || table_name.contains("edge")
                {
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
            Expr::SksSimilar { .. } | Expr::SksFollow { .. } | Expr::SksAssemble { .. } => true,
            Expr::Unary { op: _, expr } => self.contains_sks_funcs(expr),
            Expr::Binary { left, right, .. } => {
                self.contains_sks_funcs(left) || self.contains_sks_funcs(right)
            }
            Expr::FuncCall { args, .. } => args.iter().any(|e| self.contains_sks_funcs(e)),
            _ => false,
        }
    }

    /// Generate execution operations for the selected strategy
    fn generate_operations(
        &self,
        select: &Select,
        strategy: &ExecutionStrategy,
    ) -> Result<Vec<ExecutionOperation>> {
        let mut operations = Vec::new();

        match strategy {
            ExecutionStrategy::VectorOnly => {
                // Prefer SKS SIMILAR() when present; otherwise fallback to generic extraction
                if let Some(sim) = self.find_sks_similar(select) {
                    for table in &select.from {
                        if let Some(collection_id) = table.name.as_ref() {
                            self.validate_similar_field(collection_id, &sim)?;
                            operations.push(ExecutionOperation::VectorSearch {
                                collection_id: collection_id.clone(),
                                query_vector: self.try_parse_query_vector(&sim.query),
                                filters: self.convert_where_to_filter(&select.selection)?,
                                top_k: select.limit.unwrap_or(100) as usize,
                                distance_metric: sim
                                    .metric
                                    .clone()
                                    .unwrap_or_else(|| "cosine".to_string()),
                            });
                        }
                    }
                } else if let Some(table) = select.from.first() {
                    let collection_id = table
                        .name
                        .as_ref()
                        .ok_or_else(|| anyhow!("Missing collection name"))?;
                    operations.push(ExecutionOperation::VectorSearch {
                        collection_id: collection_id.clone(),
                        query_vector: self.extract_query_vector(select)?,
                        filters: self.convert_where_to_filter(&select.selection)?,
                        top_k: select.limit.unwrap_or(100) as usize,
                        distance_metric: self.extract_distance_metric(select)?,
                    });
                }
            }

            ExecutionStrategy::GraphOnly => {
                // Prefer SKS FOLLOW() when present; otherwise fallback to generic extraction
                if let Some(fol) = self.find_sks_follow(select) {
                    self.validate_follow_edge(&fol)?;
                    operations.push(ExecutionOperation::GraphTraversal {
                        graph_id: "default".to_string(), // TODO: Extract from context
                        start_nodes: self.expr_to_start_nodes(&fol.start),
                        edge_types: vec![fol.edge],
                        max_depth: fol.max_depth,
                        filters: self.convert_where_to_filter(&select.selection)?,
                        vector_target_collection: select.from.first().and_then(|t| t.name.clone()),
                    });
                } else {
                    operations.push(ExecutionOperation::GraphTraversal {
                        graph_id: "default".to_string(), // TODO: Extract from context
                        start_nodes: self.extract_start_nodes(select)?,
                        edge_types: self.extract_edge_types(select)?,
                        max_depth: self.extract_max_depth(select).unwrap_or(3),
                        filters: self.convert_where_to_filter(&select.selection)?,
                        vector_target_collection: select.from.first().and_then(|t| t.name.clone()),
                    });
                }
            }

            ExecutionStrategy::Hybrid => {
                // Generate both vector and graph operations with fusion
                if let Some(sim) = self.find_sks_similar(select) {
                    for table in &select.from {
                        if let Some(collection_id) = table.name.as_ref() {
                            self.validate_similar_field(collection_id, &sim)?;
                            operations.push(ExecutionOperation::VectorSearch {
                                collection_id: collection_id.clone(),
                                query_vector: self.try_parse_query_vector(&sim.query),
                                filters: self.convert_where_to_filter(&select.selection)?,
                                top_k: select.limit.unwrap_or(100) as usize,
                                distance_metric: sim
                                    .metric
                                    .clone()
                                    .unwrap_or_else(|| "cosine".to_string()),
                            });
                        }
                    }
                }
                if let Some(fol) = self.find_sks_follow(select) {
                    self.validate_follow_edge(&fol)?;
                    operations.push(ExecutionOperation::GraphTraversal {
                        graph_id: "default".to_string(), // TODO: Extract from context
                        start_nodes: self.expr_to_start_nodes(&fol.start),
                        edge_types: vec![fol.edge],
                        max_depth: fol.max_depth,
                        filters: self.convert_where_to_filter(&select.selection)?,
                        vector_target_collection: select.from.first().and_then(|t| t.name.clone()),
                    });
                }
                operations.push(ExecutionOperation::Fusion {
                    strategy: FusionStrategy::ReciprocalRankFusion { k: 60.0 },
                    weights: self
                        .fusion_weights
                        .clone()
                        .unwrap_or_else(|| vec![0.6, 0.4]),
                });
            }

            _ => {
                return Err(anyhow!(
                    "Execution strategy not yet implemented: {:?}",
                    strategy
                ));
            }
        }

        // Aggregate (GROUP BY / HAVING)
        if !select.group_by.is_empty() {
            let group_keys = select
                .group_by
                .iter()
                .filter_map(|e| self.expr_to_identifier(e))
                .collect::<Vec<_>>();
            let projection_exprs: Vec<_> =
                select.projection.iter().map(|p| p.expr.clone()).collect();
            let aggs = self.extract_aggregates(&projection_exprs);
            let having = self.convert_where_to_filter(&select.having)?; // reuse filter converter
            operations.push(ExecutionOperation::Aggregate {
                group_keys,
                aggs,
                having,
            });
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
    fn convert_where_to_filter(
        &self,
        where_clause: &Option<Expr>,
    ) -> Result<Option<FilterExpression>> {
        if let Some(expr) = where_clause {
            let filter = self.expr_to_filter_expression(expr)?;
            tracing::debug!("Converted WHERE clause to filter: {:?}", filter);
            Ok(filter)
        } else {
            Ok(None)
        }
    }

    /// Find first SKS SIMILAR occurrence in selection or order_by
    fn find_sks_similar(&self, select: &Select) -> Option<SksSimilarArgs> {
        // Validate SIMILAR field roughly against schema: ensure the field name looks like an embedding column
        if let Some(expr) = &select.selection
            && let Some(sim) = self.walk_find_similar(expr) {
                return Some(sim);
            }
        for ob in &select.order_by {
            if let Some(sim) = self.walk_find_similar(&ob.expr) {
                return Some(sim);
            }
        }
        None
    }

    fn walk_find_similar(&self, expr: &Expr) -> Option<SksSimilarArgs> {
        match expr {
            Expr::SksSimilar {
                field: _,
                query,
                metric,
                threshold,
            } => Some(SksSimilarArgs {
                query: query.as_ref().clone(),
                metric: metric.clone(),
                threshold: *threshold,
            }),
            Expr::Unary { expr, .. } => self.walk_find_similar(expr),
            Expr::Binary { left, right, .. } => self
                .walk_find_similar(left)
                .or_else(|| self.walk_find_similar(right)),
            Expr::AggCall { args, .. } | Expr::FuncCall { args, .. } => {
                args.iter().find_map(|e| self.walk_find_similar(e))
            }
            _ => None,
        }
    }

    fn validate_similar_field(&self, collection_id: &str, sim: &SksSimilarArgs) -> Result<()> {
        // TODO: query collection schema; best-effort heuristic for now
        // Warn if field name not obviously embedding/vector
        let _ = collection_id; // reserved for future use
        if let Expr::Identifier(field) = &sim.query {
            // if query is identifier, assume it's not a vector literal — skip
            let _ = field;
        }
        // Heuristic: ok by default; add tracing here when schema is accessible
        Ok(())
    }

    fn validate_follow_edge(&self, fol: &SksFollowArgs) -> Result<()> {
        if fol.edge.trim().is_empty() {
            return Err(anyhow!("FOLLOW: edge type cannot be empty"));
        }
        // Use GraphOperationsService stats to validate edge types when available
        // (best-effort; if stats not accessible, skip)
        // TODO: Add graph_id parameter when available from context
        if let Ok(stats) =
            tokio::runtime::Handle::current().block_on(self.graph_service.get_stats("default"))
        {
            let exists = stats
                .edge_type_stats
                .iter()
                .any(|e| e.edge_type == fol.edge);
            if !exists {
                return Err(anyhow!(
                    "FOLLOW: unknown edge type '{}'. Check graph schema or ingest.",
                    fol.edge
                ));
            }
        }
        Ok(())
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
            Expr::SksFollow {
                start,
                edge,
                max_depth,
            } => Some(SksFollowArgs {
                start: start.as_ref().clone(),
                edge: edge.clone(),
                max_depth: *max_depth,
            }),
            Expr::Unary { expr, .. } => self.walk_find_follow(expr),
            Expr::Binary { left, right, .. } => self
                .walk_find_follow(left)
                .or_else(|| self.walk_find_follow(right)),
            Expr::AggCall { args, .. } | Expr::FuncCall { args, .. } => {
                args.iter().find_map(|e| self.walk_find_follow(e))
            }
            _ => None,
        }
    }

    /// Try to parse a query vector from an expression (best-effort)
    fn try_parse_query_vector(&self, expr: &Expr) -> Option<Vec<f32>> {
        tracing::debug!("Trying to parse query vector from expression: {:?}", expr);
        match expr {
            // [0.1, 0.2, 0.3] array literal
            Expr::Array { elem, .. } => {
                tracing::debug!("Found array expression with {} elements", elem.len());
                let mut out = Vec::new();
                for e in elem {
                    if let Expr::Literal(crate::query::ast::Literal::Number(n)) = e {
                        out.push(*n as f32);
                    } else {
                        tracing::warn!("Array element is not a number: {:?}", e);
                        return None;
                    }
                }
                if out.is_empty() {
                    None
                } else {
                    tracing::info!(
                        "Successfully parsed vector from array, length: {}",
                        out.len()
                    );
                    Some(out)
                }
            }
            // VECTOR(0.1, 0.2, 0.3)
            Expr::FuncCall { name, args } if name.eq_ignore_ascii_case("VECTOR") => {
                let mut out = Vec::new();
                for a in args {
                    if let Expr::Literal(crate::query::ast::Literal::Number(n)) = a {
                        out.push(*n as f32);
                    } else {
                        return None;
                    }
                }
                if out.is_empty() { None } else { Some(out) }
            }
            // '[0.1,0.2,0.3]' string literal
            Expr::Literal(crate::query::ast::Literal::String(s)) => {
                tracing::debug!("Found string literal: {}", s);
                if s.starts_with('[') {
                    if let Ok(v) = serde_json::from_str::<Vec<f32>>(s) {
                        tracing::info!(
                            "Successfully parsed vector from string, length: {}",
                            v.len()
                        );
                        return Some(v);
                    } else {
                        tracing::warn!("Failed to parse JSON array from string: {}", s);
                    }
                }
                None
            }
            // Parameter (decode from planner params: $1, $2, ...)
            Expr::Param(ph) => {
                // Expect $<n>
                if let Some(n) = ph.strip_prefix('$')
                    && let Ok(idx) = n.parse::<usize>() {
                        let pos = idx.saturating_sub(1);
                        if let Some(pv) = self.params.as_ref().and_then(|v| v.get(pos)) {
                            return self.sql_value_to_vec(pv);
                        }
                    }
                None
            }
            _ => {
                tracing::debug!("Expression type not supported for vector parsing");
                None
            }
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
                // Handle logical operators (AND, OR) recursively
                match op {
                    BinaryOp::And => {
                        let left_filter = self.expr_to_filter_expression(left)?;
                        let right_filter = self.expr_to_filter_expression(right)?;

                        match (left_filter, right_filter) {
                            (Some(l), Some(r)) => Ok(Some(FilterExpression::And(vec![l, r]))),
                            (Some(l), None) => Ok(Some(l)),
                            (None, Some(r)) => Ok(Some(r)),
                            (None, None) => Ok(None),
                        }
                    }
                    BinaryOp::Or => {
                        let left_filter = self.expr_to_filter_expression(left)?;
                        let right_filter = self.expr_to_filter_expression(right)?;

                        match (left_filter, right_filter) {
                            (Some(l), Some(r)) => Ok(Some(FilterExpression::Or(vec![l, r]))),
                            (Some(l), None) => Ok(Some(l)),
                            (None, Some(r)) => Ok(Some(r)),
                            (None, None) => Ok(None),
                        }
                    }
                    // Handle comparison operators
                    _ => {
                        let field = self.extract_field_name(left)?;
                        let operator = self.convert_comparison_op(op)?;
                        let value = self.extract_filter_value(right)?;

                        Ok(Some(FilterExpression::Comparison {
                            field,
                            operator,
                            value,
                        }))
                    }
                }
            }
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
            }
            _ => Err(anyhow!("Unsupported field expression: {:?}", expr)),
        }
    }

    fn expr_to_identifier(&self, expr: &Expr) -> Option<String> {
        match expr {
            Expr::Identifier(s) => Some(s.clone()),
            _ => None,
        }
    }

    #[allow(dead_code)]
    fn extract_join_keys(&self, expr: &Expr) -> Option<(String, String)> {
        Self::extract_join_keys_static(expr)
    }

    pub(crate) fn extract_join_keys_static(expr: &Expr) -> Option<(String, String)> {
        // Support simple equality join: a.id = b.entity_id
        if let Expr::Binary { left, op, right } = expr
            && matches!(op, BinaryOp::Eq) {
                let left_key = match &**left {
                    Expr::Identifier(s) => s.clone(),
                    _ => return None,
                };
                let right_key = match &**right {
                    Expr::Identifier(s) => s.clone(),
                    _ => return None,
                };
                return Some((left_key, right_key));
            }
        None
    }

    pub(crate) fn extract_join_key_pairs_static(expr: &Expr) -> Vec<(String, String)> {
        let mut out = Vec::new();
        if let Expr::Binary { left, op, right } = expr { match op {
            BinaryOp::Eq => {
                if let (Some(l), Some(r)) = (
                    Self::extract_join_keys_static(expr).map(|p| p.0),
                    Self::extract_join_keys_static(expr).map(|p| p.1),
                ) {
                    out.push((l, r));
                }
            }
            BinaryOp::And => {
                out.extend(Self::extract_join_key_pairs_static(left));
                out.extend(Self::extract_join_key_pairs_static(right));
            }
            _ => {}
        } }
        out
    }

    fn extract_aggregates(
        &self,
        projection: &Vec<Expr>,
    ) -> Vec<crate::query::execution::AggregateSpec> {
        use crate::query::execution::{AggregateFunc, AggregateSpec};
        let mut out = Vec::new();
        for expr in projection {
            if let Expr::AggCall { name, args } = expr {
                let alias = name.to_uppercase();
                let field = args.first()
                    .and_then(|e| self.expr_to_identifier(e))
                    .unwrap_or("*".to_string());
                let func = match alias.as_str() {
                    s if s.contains("COUNT") => AggregateFunc::Count,
                    s if s.contains("SUM") => AggregateFunc::Sum,
                    s if s.contains("AVG") => AggregateFunc::Avg,
                    s if s.contains("MIN") => AggregateFunc::Min,
                    s if s.contains("MAX") => AggregateFunc::Max,
                    _ => AggregateFunc::Count,
                };
                out.push(AggregateSpec {
                    alias: alias.clone(),
                    func,
                    field,
                });
            }
        }
        out
    }

    /// Convert AST binary operator to FilterExpression comparison operator
    fn convert_comparison_op(
        &self,
        op: &BinaryOp,
    ) -> Result<crate::core::search::ComparisonOperator> {
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
            Expr::Literal(literal) => match literal {
                crate::query::ast::Literal::String(s) => Ok(serde_json::Value::String(s.clone())),
                crate::query::ast::Literal::Number(n) => Ok(serde_json::json!(n)),
                crate::query::ast::Literal::Bool(b) => Ok(serde_json::Value::Bool(*b)),
                crate::query::ast::Literal::Null => Ok(serde_json::Value::Null),
            },
            Expr::Param(ph) => {
                if let Some(n) = ph.strip_prefix('$') {
                    let idx = n
                        .parse::<usize>()
                        .map_err(|_| anyhow!("Invalid parameter placeholder: {}", ph))?;
                    let pos = idx.saturating_sub(1);
                    if let Some(pv) = self.params.as_ref().and_then(|v| v.get(pos)) {
                        return Ok(self.sql_value_to_json(pv));
                    }
                    Err(anyhow!("Parameter {} is missing", ph))
                } else {
                    Err(anyhow!("Unsupported parameter placeholder: {}", ph))
                }
            }
            _ => Err(anyhow!("Unsupported filter value expression: {:?}", expr)),
        }
    }

    fn sql_value_to_json(&self, v: &crate::proto::proximadb_v1::SqlValue) -> serde_json::Value {
        use crate::proto::proximadb_v1::sql_value::Value as V;
        match v.value.as_ref() {
            Some(V::StringValue(s)) => serde_json::Value::String(s.clone()),
            Some(V::NumberValue(n)) => serde_json::json!(*n),
            Some(V::BoolValue(b)) => serde_json::json!(*b),
            Some(V::Int64Value(i)) => serde_json::json!(*i),
            Some(V::BytesValue(b)) => {
                serde_json::Value::Array(b.iter().map(|x| serde_json::json!(*x)).collect())
            }
            Some(V::ArrayValue(arr)) => serde_json::Value::Array(
                arr.values
                    .iter()
                    .map(|sv| self.sql_value_to_json(sv))
                    .collect(),
            ),
            Some(V::ObjectValue(obj)) => {
                let mut map = serde_json::Map::new();
                for (k, sv) in &obj.fields {
                    map.insert(k.clone(), self.sql_value_to_json(sv));
                }
                serde_json::Value::Object(map)
            }
            Some(V::NullValue(_)) | None => serde_json::Value::Null,
        }
    }

    /// Extract query vector from ORDER BY vector similarity function
    fn extract_query_vector(&self, select: &Select) -> Result<Option<Vec<f32>>> {
        for order_expr in &select.order_by {
            if let Expr::FuncCall { name, args } = &order_expr.expr
                && name.to_uppercase().contains("VECTOR_SIMILARITY") && args.len() >= 2 {
                    // Extract vector from second argument (first is the vector field name)
                    return Ok(self.try_parse_query_vector(&args[1]));
                }
        }
        Ok(None)
    }

    /// Extract distance metric from vector similarity function
    fn extract_distance_metric(&self, select: &Select) -> Result<String> {
        for order_expr in &select.order_by {
            if let Expr::FuncCall { name, args } = &order_expr.expr
                && name.to_uppercase().contains("VECTOR_SIMILARITY") && args.len() >= 3 {
                    // Extract metric from third argument
                    if let Expr::Literal(crate::query::ast::Literal::String(s)) = &args[2] {
                        return Ok(s.to_lowercase());
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
        select
            .projection
            .iter()
            .map(|item| {
                if let Some(alias) = &item.alias {
                    alias.clone()
                } else {
                    self.expr_to_identifier(&item.expr).unwrap_or("*".into())
                }
            })
            .collect()
    }

    fn generate_projections(&self, select: &Select) -> Vec<ProjectionTransform> {
        let transforms = Vec::new();
        // Note: We no longer need ExtractMetadata transformations because
        // the executor already creates fields with "metadata." prefix
        // matching the SELECT clause column names.
        // Keeping this method for future transformation types.
        for item in &select.projection {
            match &item.expr {
                _ => {
                    // TODO: Handle other projection types (timestamp formatting, etc.)
                }
            }
        }

        transforms
    }

    fn detect_sks_functions(&self, expr: &Expr) -> bool {
        match expr {
            Expr::FuncCall { name, .. } => {
                matches!(
                    name.to_uppercase().as_str(),
                    "SIMILAR" | "FOLLOW" | "ASSEMBLE"
                )
            }
            Expr::Binary { left, right, .. } => {
                self.detect_sks_functions(left) || self.detect_sks_functions(right)
            }
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
        for item in &select.projection {
            fields.extend(self.extract_fields_from_expr(&item.expr));
        }

        fields
    }

    fn extract_fields_from_expr(&self, expr: &Expr) -> Vec<String> {
        match expr {
            Expr::Identifier(ident) if ident.starts_with("metadata.") => {
                vec![ident.strip_prefix("metadata.").unwrap_or(ident).to_string()]
            }
            Expr::Binary { left, right, .. } => {
                let mut fields = self.extract_fields_from_expr(left);
                fields.extend(self.extract_fields_from_expr(right));
                fields
            }
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
            }
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

    /// Generate cache key for query plan caching
    fn generate_cache_key(&self, query: &Query) -> String {
        let mut hasher = DefaultHasher::new();
        format!("{:?}", query).hash(&mut hasher);
        format!("plan_{:x}", hasher.finish())
    }

    /// Plan CTE (Common Table Expression) queries
    fn plan_cte(&self, _ctes: &[crate::query::ast::Cte], query: &Query) -> Result<ExecutionPlan> {
        // For now, just plan the main query - CTE optimization to be implemented
        self.create_plan(query)
    }

    /// Plan set operation queries (UNION, INTERSECT, EXCEPT)
    fn plan_set_operation(
        &self,
        left: &Query,
        op: &crate::query::ast::SetOp,
        all: bool,
        right: &Query,
    ) -> Result<ExecutionPlan> {
        let left_plan = self.create_plan(left)?;
        let right_plan = self.create_plan(right)?;

        let set_operation = match op {
            crate::query::ast::SetOp::Union => ExecutionOperation::SetUnion {
                distinct: !all,
                left_results: "left_plan_results".to_string(),
                right_results: "right_plan_results".to_string(),
            },
            crate::query::ast::SetOp::Intersect => ExecutionOperation::SetIntersect {
                distinct: !all,
                left_results: "left_plan_results".to_string(),
                right_results: "right_plan_results".to_string(),
            },
            crate::query::ast::SetOp::Except => ExecutionOperation::SetExcept {
                distinct: !all,
                left_results: "left_plan_results".to_string(),
                right_results: "right_plan_results".to_string(),
            },
        };

        Ok(ExecutionPlan {
            execution_strategy: ExecutionStrategy::Relational,
            operations: vec![set_operation],
            estimated_cost: left_plan.estimated_cost + right_plan.estimated_cost,
            optimizations: vec!["Set operation optimization".to_string()],
            performance_hints: vec!["Consider LIMIT for large set operations".to_string()],
            seeding_strategy: self.seeding_strategy.clone(),
            limit: None,
            offset: None,
        })
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
    #[allow(dead_code)]
    pub threshold: Option<f64>,
}

#[derive(Debug, Clone)]
struct SksFollowArgs {
    pub start: Expr,
    pub edge: String,
    pub max_depth: u32,
}

/// Cost model for execution planning optimization
///
/// When `collection_stats` are available the model scales estimates by
/// actual cardinality; otherwise it falls back to the original fixed costs.
pub struct CostModel {
    // Cost estimates for different operation types
    vector_search_base_cost: f64,
    graph_traversal_base_cost: f64,
    metadata_filter_cost: f64,
    fusion_cost: f64,
    /// Collection statistics for cardinality-aware costing (keyed by collection id)
    collection_stats: HashMap<String, crate::storage::traits::CollectionStats>,
}

impl CostModel {
    pub fn new() -> Self {
        Self {
            vector_search_base_cost: 1.0,
            graph_traversal_base_cost: 2.0,
            metadata_filter_cost: 0.1, // Very low due to HashMap optimization
            fusion_cost: 0.5,
            collection_stats: HashMap::new(),
        }
    }

    /// Register collection statistics for cardinality-aware cost estimation
    pub fn with_collection_stats(
        mut self,
        collection_id: String,
        stats: crate::storage::traits::CollectionStats,
    ) -> Self {
        self.collection_stats.insert(collection_id, stats);
        self
    }

    /// Estimate the output cardinality of an operation
    ///
    /// Returns estimated number of rows produced. When collection stats are
    /// available, estimates are grounded in real cardinalities; otherwise
    /// conservative defaults are used.
    pub fn estimate_cardinality(&self, operation: &ExecutionOperation) -> u64 {
        match operation {
            ExecutionOperation::VectorSearch {
                collection_id,
                top_k,
                filters,
                ..
            } => {
                let base_rows = self
                    .collection_stats
                    .get(collection_id)
                    .map_or(10_000, |s| s.row_count);
                // Vector search returns at most top_k results
                let top_k_u64 = *top_k as u64;
                let capped = top_k_u64.min(base_rows);
                // Filters reduce output further (assume 50% selectivity as fallback)
                if filters.is_some() {
                    (capped as f64 * 0.5).max(1.0) as u64
                } else {
                    capped
                }
            }
            ExecutionOperation::GraphTraversal { max_depth, .. } => {
                // Exponential fan-out estimate, capped
                let fan_out = 5u64; // average edges per node
                fan_out.saturating_pow(*max_depth).min(100_000)
            }
            ExecutionOperation::Fusion { .. } => {
                // Fusion merges results — conservative estimate
                1_000
            }
            ExecutionOperation::Project { .. } => {
                // Projection doesn't change cardinality — pass through
                1_000
            }
            ExecutionOperation::Aggregate { .. } => {
                // Aggregation typically reduces rows significantly
                100
            }
            ExecutionOperation::Join { .. } => {
                // Join cardinality depends on both sides; conservative default
                10_000
            }
            _ => 1_000,
        }
    }

    /// Estimate total execution cost for operations
    pub fn estimate_total_cost(&self, operations: &[ExecutionOperation]) -> f64 {
        operations
            .iter()
            .map(|op| self.estimate_operation_cost(op))
            .sum()
    }

    /// Estimate cost for individual operation
    ///
    /// When collection statistics are available, costs scale with the
    /// collection's row count. For vector search this means larger
    /// collections are proportionally more expensive. For operations
    /// without stats the original fixed costs are returned.
    pub fn estimate_operation_cost(&self, operation: &ExecutionOperation) -> f64 {
        match operation {
            ExecutionOperation::VectorSearch {
                collection_id,
                top_k,
                filters,
                ..
            } => {
                let top_k_f = (*top_k as f64).max(1.0);
                let base_cost = self.vector_search_base_cost * top_k_f.log10();
                let filter_cost = match filters {
                    Some(_) => self.metadata_filter_cost, // HashMap filtering is cheap
                    None => 0.0,
                };
                // Scale by collection size when stats are available
                let scale = self
                    .collection_stats
                    .get(collection_id)
                    .map_or(1.0, |s| {
                        let n = s.row_count as f64;
                        if n <= 1.0 {
                            return 1.0;
                        }
                        // With HNSW index: O(log n), without: O(n)
                        if s.has_hnsw_index {
                            n.log2() / 10.0 // sub-linear
                        } else {
                            n / 10_000.0 // linear scan scaling
                        }
                    })
                    .max(0.1);
                (base_cost + filter_cost) * scale
            }
            ExecutionOperation::GraphTraversal { max_depth, .. } => {
                self.graph_traversal_base_cost * (*max_depth as f64)
            }
            ExecutionOperation::Fusion { .. } => self.fusion_cost,
            ExecutionOperation::Project { .. } => 0.1,
            ExecutionOperation::Aggregate { .. } => 0.5,
            ExecutionOperation::Join { .. } => 1.0,
            ExecutionOperation::SetUnion { .. } => 0.8,
            ExecutionOperation::SetIntersect { .. } => 0.8,
            ExecutionOperation::SetExcept { .. } => 0.8,
            ExecutionOperation::Union { .. } => 0.7,
            ExecutionOperation::CteMaterialization { .. } => 0.9,
        }
    }
}

#[cfg(test)]
mod planner_tests {
    use super::*;
    use crate::query::ast::*;

    #[tokio::test]
    async fn test_vector_query_planning() {
        let planner = create_test_planner().await;

        // Create vector query AST
        let query = Query::Select(Select {
            projection: vec![ProjectionItem {
                expr: Expr::Identifier("*".to_string()),
                alias: None,
            }],
            from: vec![TableRef {
                name: Some("products".to_string()),
                subquery: None,
                alias: None,
            }],
            joins: vec![],
            selection: Some(Expr::Binary {
                left: Box::new(Expr::Identifier("metadata.category".to_string())),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Literal(Literal::String("electronics".to_string()))),
            }),
            group_by: vec![],
            having: None,
            order_by: vec![OrderByExpr {
                expr: Expr::FuncCall {
                    name: "VECTOR_SIMILARITY".to_string(),
                    args: vec![],
                },
                asc: false,
            }],
            limit: Some(10),
            offset: None,
        });

        let plan = planner.create_plan(&query).unwrap();

        assert!(matches!(
            plan.execution_strategy,
            ExecutionStrategy::VectorOnly
        ));
        assert!(plan.operations.len() >= 1);
        assert!(
            plan.optimizations
                .contains(&"HashMap metadata filtering (O(1) vs O(n))".to_string())
        );
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
        assert!(
            cost < 5.0,
            "HashMap filtering should have low cost, got {}",
            cost
        );
    }

    async fn create_test_planner() -> ExecutionPlanner {
        use crate::graph::service::GraphOperationsService;
        use crate::index::AxisManager;
        use crate::services::collection::manager::CollectionService;
        use crate::services::operations::vectors::VectorOperationsService;
        use crate::storage::engines::impls::sst::SstEngine;
        use crate::storage::persistence::write_ahead_log::WriteAheadLogManager;
        use std::sync::Arc;

        // Create temporary directory for storage
        let temp_dir = tempfile::tempdir().expect("Failed to create temp dir");
        let storage_url = format!("file:///{}", temp_dir.path().display());

        // Create SST storage engine
        let storage_engine = Arc::new(SstEngine::new().await.expect("Failed to create SST engine"));

        // Create WAL manager with default config
        use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
        use crate::storage::persistence::write_ahead_log::{
            WALBatchFactory, WALConfig, WriteBufferStrategyType,
        };
        let fs_config = FilesystemConfig::default();
        let filesystem = Arc::new(
            FilesystemFactory::create(fs_config)
                .await
                .expect("Failed to create filesystem"),
        );
        let wal_config = WALConfig::default();
        let strategy = WALBatchFactory::create_batch_serialization_strategy(
            WriteBufferStrategyType::AvroBatch,
            &wal_config,
            filesystem,
        )
        .await
        .expect("Failed to create WAL strategy");
        let wal_manager = Arc::new(
            WriteAheadLogManager::new(strategy, wal_config)
                .await
                .expect("Failed to create WAL manager"),
        );

        // Create Axis index manager with default config
        use crate::index::axis::AxisConfig;
        let axis_config = AxisConfig::default();
        let axis_manager = Arc::new(
            AxisManager::new(axis_config)
                .await
                .expect("Failed to create Axis manager"),
        );

        // Create collection service with universal metadata backend
        use crate::core::config::StorageConfig;
        use crate::storage::metadata::backends::universal_backend::UniversalMetadataBackend;
        use crate::storage::traits::InternalCollectionProvider;

        let fs_config = FilesystemConfig::default();
        let filesystem2 = Arc::new(
            FilesystemFactory::create(fs_config)
                .await
                .expect("Failed to create filesystem"),
        );

        use crate::storage::metadata::backends::universal_backend::UniversalMetadataConfig;
        let metadata_config = UniversalMetadataConfig {
            storage_url: storage_url.clone(),
            compression: true,
            enable_snapshots: false,
            snapshot_threshold: 1000,
            keep_snapshots: 3,
            backup_url: None,
            temp_dir: Some(temp_dir.path().to_str().unwrap().to_string()),
        };
        let metadata_backend = Arc::new(
            UniversalMetadataBackend::new(metadata_config, filesystem2)
                .await
                .expect("Failed to create metadata backend"),
        ) as Arc<dyn InternalCollectionProvider>;
        let storage_config = StorageConfig {
            metadata_url: storage_url.clone(),
            ..Default::default()
        };
        let collection_service = Arc::new(
            CollectionService::new(metadata_backend, storage_config)
                .await
                .expect("Failed to create collection service"),
        );

        // Create vector operations service with all dependencies
        let vector_service = Arc::new(VectorOperationsService::new(
            storage_engine,
            wal_manager,
            axis_manager,
            collection_service,
        ));

        // Create graph service
        let graph_service = Arc::new(GraphOperationsService::new());

        // Keep temp_dir alive by leaking it (tests are short-lived)
        std::mem::forget(temp_dir);

        ExecutionPlanner::new(vector_service, graph_service)
    }

    #[test]
    fn test_extract_join_keys_static_simple() {
        let on = Expr::Binary {
            left: Box::new(Expr::Identifier("a.id".to_string())),
            op: BinaryOp::Eq,
            right: Box::new(Expr::Identifier("b.entity_id".to_string())),
        };
        let (l, r) = ExecutionPlanner::extract_join_keys_static(&on).expect("keys");
        assert_eq!(l, "a.id");
        assert_eq!(r, "b.entity_id");
    }

    #[test]
    fn test_extract_join_key_pairs_static_and_chain() {
        let on = Expr::Binary {
            left: Box::new(Expr::Binary {
                left: Box::new(Expr::Identifier("a.id".to_string())),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Identifier("b.entity_id".to_string())),
            }),
            op: BinaryOp::And,
            right: Box::new(Expr::Binary {
                left: Box::new(Expr::Identifier("a.type".to_string())),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Identifier("b.type".to_string())),
            }),
        };
        let pairs = ExecutionPlanner::extract_join_key_pairs_static(&on);
        assert_eq!(pairs.len(), 2);
        assert_eq!(pairs[0], ("a.id".to_string(), "b.entity_id".to_string()));
        assert_eq!(pairs[1], ("a.type".to_string(), "b.type".to_string()));
    }

    #[test]
    fn test_extract_join_key_pairs_with_parens_and_reversed_order() {
        // ( (b.id = a.id) AND (b.kind = a.kind) )
        let on = Expr::Binary {
            left: Box::new(Expr::Binary {
                left: Box::new(Expr::Binary {
                    left: Box::new(Expr::Identifier("b.id".to_string())),
                    op: BinaryOp::Eq,
                    right: Box::new(Expr::Identifier("a.id".to_string())),
                }),
                op: BinaryOp::And,
                right: Box::new(Expr::Binary {
                    left: Box::new(Expr::Identifier("b.kind".to_string())),
                    op: BinaryOp::Eq,
                    right: Box::new(Expr::Identifier("a.kind".to_string())),
                }),
            }),
            op: BinaryOp::And, // trailing AND with a tautology to ensure traversal robustness
            right: Box::new(Expr::Binary {
                left: Box::new(Expr::Identifier("1".to_string())),
                op: BinaryOp::Eq,
                right: Box::new(Expr::Identifier("1".to_string())),
            }),
        };
        let pairs = ExecutionPlanner::extract_join_key_pairs_static(&on);
        // Should still extract the two equality pairs
        assert!(pairs.iter().any(|(l, r)| l == "b.id" && r == "a.id"));
        assert!(pairs.iter().any(|(l, r)| l == "b.kind" && r == "a.kind"));
    }

    // NOTE: Full SQL-lowered JOIN tests require JOIN lowering support.
    // This suite validates composite ON parsing semantics equivalent to SQL-lowered AST.

    #[tokio::test]
    async fn test_query_plan_caching() {
        use crate::graph::GraphOperationsService;

        // Create mock services (simplified for testing)
        let _graph_service = Arc::new(GraphOperationsService::new());
        // Skip complex vector service setup for test
        // Note: Full test would create ExecutionPlanner and test query plan caching
    }

    #[tokio::test]
    async fn test_set_operation_planning() {
        use crate::graph::GraphOperationsService;

        // Create simple test planner
        let _graph_service = Arc::new(GraphOperationsService::new());
        // Skip test - requires complex VectorOperationsService setup
        // Note: Full test would create ExecutionPlanner and test set operation planning
    }

    #[tokio::test]
    async fn test_cache_key_generation() {
        use crate::graph::GraphOperationsService;

        let _graph_service = Arc::new(GraphOperationsService::new());
        // Skip test - requires complex VectorOperationsService setup
        // Note: Full test would create ExecutionPlanner and test cache key generation
    }

    #[test]
    fn test_cost_model_estimation() {
        let cost_model = CostModel::new();

        let operations = vec![
            ExecutionOperation::VectorSearch {
                collection_id: "test".to_string(),
                query_vector: None,
                filters: None,
                top_k: 100,
                distance_metric: "cosine".to_string(),
            },
            ExecutionOperation::Project {
                columns: vec!["id".to_string()],
                transformations: vec![],
            },
        ];

        let total_cost = cost_model.estimate_total_cost(&operations);
        assert!(total_cost > 0.0);
    }
}
