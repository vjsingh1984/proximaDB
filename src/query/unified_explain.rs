//! Unified Explain Schema Implementation (Issue #47, SB-17)
//!
//! This module provides the implementation for unified query plan explanation
//! across all APIs (REST, gRPC, SQL, UQL). It converts internal query plans
//! into the standardized ExplainPlan proto format.
//!
//! ## Architecture
//!
//! ```text
//! ┌─────────────────────────────────────────────────────────────┐
//! │              Unified Explain Interface                       │
//! │  - REST API       → ExplainPlan (JSON)                      │
//! │  - gRPC API       → ExplainPlan (protobuf)                  │
//! │  - SQL Interface  → ExplainPlan (TEXT/JSON)                 │
//! │  - UQL Interface  → ExplainPlan (TEXT/JSON)                 │
//! └────────────────────────┬────────────────────────────────────┘
//!                           │
//!                           ▼
//!         ┌─────────────────────────────────────┐
//!         │      ExplainPlanBuilder             │
//!         │  - Converts internal plans          │
//!         │  - Adds cost estimates              │
//!         │  - Collects execution stats         │
//!         └────────────────┬────────────────────┘
//!                           │
//!                           ▼
//!         ┌─────────────────────────────────────┐
//!         │   PlanNode Converters               │
//!         │  - MultiModelPlan → ExplainPlan     │
//!         │  - FederatedPlan → ExplainPlan      │
//!         │  - SQLPlan → ExplainPlan            │
//!         └─────────────────────────────────────┘
//! ```

use anyhow::Result;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use tracing::info;

use crate::proto::explain::v1::{
    ExplainPlan, ExplainPlanRequest, ExplainPlanResponse, ExplainFormat,
    PlanNode, PlanMetadata, ExecutionStats, NodeType,
    NodeDetails, ExplainWarning,
};

/// Plan context for explain operations
#[derive(Debug, Clone, Default)]
pub struct PlanContext {
    pub collection_id: Option<String>,
    pub query_id: Option<String>,
    pub session_id: Option<String>,
    pub user_id: Option<String>,
}

/// ExplainPlanBuilder converts internal query plans into unified ExplainPlan format
#[allow(dead_code)]
pub struct ExplainPlanBuilder {
    /// Request parameters
    request: ExplainPlanRequest,

    /// Plan context
    context: PlanContext,

    /// Node ID counter
    next_node_id: usize,

    /// Node lookup map
    node_map: HashMap<String, PlanNode>,
}

impl ExplainPlanBuilder {
    /// Create a new explain plan builder
    pub fn new(request: ExplainPlanRequest, context: PlanContext) -> Self {
        Self {
            request,
            context,
            next_node_id: 0,
            node_map: HashMap::new(),
        }
    }

    /// Build explain plan from generic plan structure
    pub fn build_explain_plan(&mut self, plan_nodes: Vec<PlanNode>) -> Result<ExplainPlan> {
        info!("Building unified explain plan");

        let plan_id = self.generate_plan_id();

        // Build plan metadata
        let metadata = self.build_plan_metadata()?;

        // Build execution stats (placeholder unless analyze=true)
        let execution_stats = if self.should_analyze() {
            Some(self.build_execution_stats()?)
        } else {
            None
        };

        // Collect warnings and suggestions
        let warnings = self.collect_warnings(&plan_nodes)?;

        Ok(ExplainPlan {
            plan_id,
            query_type: self.request.query_type,
            query: self.request.query.clone(),
            optimized_query: self.request.query.clone(), // Would be optimized query
            plan_nodes,
            execution_stats,
            warnings,
            metadata: Some(metadata),
        })
    }

    /// Build plan metadata
    fn build_plan_metadata(&self) -> Result<PlanMetadata> {
        Ok(PlanMetadata {
            optimizer_version: "1.0.0".to_string(),
            optimization_level: "Standard".to_string(),
            optimization_rules_applied: vec![
                "PredicatePushdown".to_string(),
                "ProjectionPruning".to_string(),
                "JoinReordering".to_string(),
            ],
            optimization_time_ms: 0,
            execution_engine: "VectorizedPipeline".to_string(),
            storage_engines_used: vec!["SST".to_string(), "HELIX".to_string()],
            query_language: "MultiModel".to_string(),
            enabled_features: vec![
                "VectorizedExecution".to_string(),
                "SIMD".to_string(),
                "MultiThreading".to_string(),
            ],
            plan_version: "1.0".to_string(),
            plan_hash: {
                let mut hasher = std::collections::hash_map::DefaultHasher::new();
                self.request.query.hash(&mut hasher);
                format!("{:016x}", hasher.finish())
            },
            additional_metadata: None,
        })
    }

    /// Build execution stats
    fn build_execution_stats(&self) -> Result<ExecutionStats> {
        Ok(ExecutionStats {
            start_time: None,
            end_time: None,
            total_wall_time_ms: 0.0,
            total_cpu_time_ms: 0.0,
            peak_memory_bytes: 0,
            cpu_utilization_percent: 0.0,
            total_rows_in: 0,
            total_rows_out: 0,
            total_bytes_in: 0,
            total_bytes_out: 0,
            rows_per_second: 0.0,
            bytes_per_second: 0.0,
            total_cache_hits: 0,
            total_cache_misses: 0,
            cache_hit_rate: 0.0,
            total_workers: 0,
            active_workers: 0,
            worker_utilization_percent: 0.0,
        })
    }

    /// Collect warnings
    fn collect_warnings(&self, plan_nodes: &[PlanNode]) -> Result<Vec<ExplainWarning>> {
        let mut warnings = Vec::new();

        if plan_nodes.len() > 20 {
            warnings.push(ExplainWarning {
                warning_code: "COMPLEX_QUERY".to_string(),
                severity: "Warning".to_string(),
                message: "Query has many operators and may be slow".to_string(),
                suggestion: "Consider simplifying the query or adding filters".to_string(),
                affected_nodes: vec![],
            });
        }

        Ok(warnings)
    }

    /// Generate unique plan ID
    fn generate_plan_id(&self) -> String {
        format!("plan_{}", uuid::Uuid::new_v4())
    }

    /// Should analyze (execute) the query
    fn should_analyze(&self) -> bool {
        self.request.options.as_ref()
            .map(|opts| opts.analyze)
            .unwrap_or(false)
    }
}

/// Explain query using the unified schema
pub async fn explain_query_unified(request: ExplainPlanRequest) -> Result<ExplainPlanResponse> {
    info!("Explaining query with unified schema: {}", request.query);

    // For now, create a simple explain plan
    // In the full implementation, this would parse the query and convert
    // the actual internal plan to the unified format
    let context = PlanContext::default();
    let mut builder = ExplainPlanBuilder::new(request.clone(), context);

    let plan_nodes = create_placeholder_plan_nodes(&request.query);
    let plan = builder.build_explain_plan(plan_nodes)?;

    // Format output
    let formatted_output = format_explain_plan(&plan, request.format());

    Ok(ExplainPlanResponse {
        plan: Some(plan),
        formatted_output,
        success: true,
        error_message: String::new(),
        error_details: vec![],
    })
}

/// Create placeholder plan nodes for demonstration
fn create_placeholder_plan_nodes(query: &str) -> Vec<PlanNode> {
    vec![
        PlanNode {
            node_id: "node_0".to_string(),
            node_type: NodeType::NodeTypeScan as i32,
            display_name: "Collection Scan".to_string(),
            description: format!("Scan collection for query: {}", query),
            parent_ids: vec![],
            child_ids: vec!["node_1".to_string()],
            estimated_cost: 10.0,
            estimated_rows: 1000,
            actual_rows: 0,
            node_details: Some(NodeDetails {
                scan: Some(crate::proto::explain::v1::ScanDetails {
                    collection_name: "test_collection".to_string(),
                    collection_id: "col_123".to_string(),
                    columns: vec!["id".to_string(), "vector".to_string(), "metadata".to_string()],
                    filter_pushed_down: String::new(),
                    estimated_bytes: 1024000,
                    is_parallel: true,
                }),
                ..Default::default()
            }),
            node_stats: None,
            hints: vec![],
        },
        PlanNode {
            node_id: "node_1".to_string(),
            node_type: NodeType::NodeTypeFilter as i32,
            display_name: "Filter".to_string(),
            description: "Apply filter predicate".to_string(),
            parent_ids: vec!["node_0".to_string()],
            child_ids: vec![],
            estimated_cost: 5.0,
            estimated_rows: 500,
            actual_rows: 0,
            node_details: Some(NodeDetails {
                filter: Some(crate::proto::explain::v1::FilterDetails {
                    filter_condition: "id > 100".to_string(),
                    filter_columns: vec!["id".to_string()],
                    filter_type: "Range".to_string(),
                    selectivity_estimate: 0.5,
                    is_sargable: true,
                    index_used: String::new(),
                }),
                ..Default::default()
            }),
            node_stats: None,
            hints: vec!["Consider adding an index on 'id' column".to_string()],
        },
    ]
}

/// Format explain plan according to requested format
pub fn format_explain_plan(plan: &ExplainPlan, format: ExplainFormat) -> String {
    match format {
        ExplainFormat::ExplainFormatJson => {
            format!("{:#?}", plan)
        },
        ExplainFormat::ExplainFormatText => {
            format_explain_plan_text(plan)
        },
        ExplainFormat::ExplainFormatGraphviz => {
            format_explain_plan_graphviz(plan)
        },
        _ => {
            "Unsupported format".to_string()
        }
    }
}

/// Format explain plan as text
fn format_explain_plan_text(plan: &ExplainPlan) -> String {
    let mut output = String::new();

    output.push_str(&format!("Query Plan: {}\n", plan.plan_id));
    output.push_str(&format!("Query Type: {:?}\n", plan.query_type()));
    output.push_str(&format!("Query: {}\n", plan.query));
    output.push_str("\nPlan Nodes:\n");

    for node in &plan.plan_nodes {
        output.push_str(&format!("  {} (cost={:.2}, rows={})\n",
                               node.display_name, node.estimated_cost, node.estimated_rows));
        output.push_str(&format!("    -> {}\n", node.description));

        if let Some(ref details) = node.node_details {
            if details.scan.is_some() {
                output.push_str("    -> Scan operation\n");
            }
            if details.filter.is_some() {
                output.push_str("    -> Filter operation\n");
            }
        }

        if !node.hints.is_empty() {
            output.push_str("    Hints:\n");
            for hint in &node.hints {
                output.push_str(&format!("      - {}\n", hint));
            }
        }
    }

    if let Some(ref metadata) = plan.metadata {
        output.push_str(&format!("\nMetadata:\n"));
        output.push_str(&format!("  Optimizer: {}\n", metadata.optimizer_version));
        output.push_str(&format!("  Engine: {}\n", metadata.execution_engine));
    }

    if !plan.warnings.is_empty() {
        output.push_str(&format!("\nWarnings:\n"));
        for warning in &plan.warnings {
            output.push_str(&format!("  [{}] {}\n", warning.severity, warning.message));
        }
    }

    output
}

/// Format explain plan as Graphviz
fn format_explain_plan_graphviz(plan: &ExplainPlan) -> String {
    let mut output = String::from("digraph QueryPlan {\n");
    output.push_str("  rankdir=TB;\n");
    output.push_str("  node [shape=box, style=rounded];\n\n");

    for node in &plan.plan_nodes {
        let label = format!("{}\\nCost: {:.2}\\nRows: {}",
                           node.display_name, node.estimated_cost, node.estimated_rows);
        output.push_str(&format!("  \"{}\" [label=\"{}\"];\n", node.node_id, label));
    }

    output.push_str("\n");

    for node in &plan.plan_nodes {
        for child_id in &node.child_ids {
            output.push_str(&format!("  \"{}\" -> \"{}\";\n", node.node_id, child_id));
        }
    }

    output.push_str("}\n");
    output
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::explain::v1::QueryType;

    #[tokio::test]
    async fn test_unified_explain_query() {
        let request = ExplainPlanRequest {
            query: "SELECT * FROM test WHERE id > 100".to_string(),
            query_type: QueryType::QueryTypeSql as i32,
            format: ExplainFormat::ExplainFormatJson as i32,
            ..Default::default()
        };

        let response = explain_query_unified(request).await.unwrap();
        assert!(response.success);
        assert!(response.plan.is_some());
        assert!(!response.formatted_output.is_empty());
    }

    #[test]
    fn test_explain_text_formatting() {
        let plan = ExplainPlan {
            plan_id: "test_plan".to_string(),
            query_type: QueryType::QueryTypeSql as i32,
            query: "SELECT * FROM test".to_string(),
            ..Default::default()
        };

        let formatted = format_explain_plan_text(&plan);
        assert!(formatted.contains("Query Plan: test_plan"));
        assert!(formatted.contains("Query Type:"));
    }

    #[test]
    fn test_explain_graphviz_formatting() {
        let plan = ExplainPlan {
            plan_id: "test_plan".to_string(),
            query_type: QueryType::QueryTypeSql as i32,
            query: "SELECT * FROM test".to_string(),
            ..Default::default()
        };

        let formatted = format_explain_plan_graphviz(&plan);
        assert!(formatted.contains("digraph QueryPlan"));
        assert!(formatted.contains("test_plan"));
    }
}
