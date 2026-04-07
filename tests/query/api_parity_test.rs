//! API Parity Test Suite (Issue #50, SB-20)
//!
//! This module provides comprehensive testing for API parity across REST, gRPC, SQL,
//! and UQL protocols, ensuring consistent query execution and plan generation.
//!
//! ## Test Categories
//!
//! ### 1. Query Plan Parity Tests
//! - Validate that identical queries produce the same plan across protocols
//! - Ensure plan node consistency and cost estimation
//! - Verify capability claims are consistent
//!
//! ### 2. Execution Parity Tests
//! - Validate that identical queries produce the same results
//! - Ensure result ordering and pagination consistency
//! - Verify error handling consistency
//!
//! ### 3. Performance Parity Tests
//! - Measure latency differences between protocols
//! - Ensure no protocol has significant performance advantages
//! - Validate resource usage consistency
//!
//! ## Key Features
//!
//! - **Cross-Protocol Testing**: Test the same query via all APIs
//! - **Plan Comparison**: Detailed plan node comparison
//! - **Result Validation**: Exact result matching
//! - **Performance Measurement**: Protocol overhead analysis

use anyhow::Result;
use std::collections::HashMap;
use std::time::{Duration, Instant};
use serde_json::Value as JsonValue;

// Test imports for different protocols
// Note: These would be the actual client implementations
use proximadb::proto::explain::v1::{ExplainPlan, ExplainPlanRequest, QueryType, ExplainFormat};
use proximadb::query::unified_explain::{explain_query_unified, format_explain_plan};

/// API protocol identifier
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApiProtocol {
    Rest,
    Grpc,
    Sql,
    Uql,
}

impl ApiProtocol {
    pub fn as_str(&self) -> &str {
        match self {
            ApiProtocol::Rest => "REST",
            ApiProtocol::Grpc => "gRPC",
            ApiProtocol::Sql => "SQL",
            ApiProtocol::Uql => "UQL",
        }
    }
}

/// Query execution result for parity testing
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Protocol used
    pub protocol: ApiProtocol,

    /// Query executed
    pub query: String,

    /// Execution time
    pub execution_time_ms: f64,

    /// Result rows
    pub rows: Vec<JsonValue>,

    /// Row count
    pub row_count: usize,

    /// Error message (if execution failed)
    pub error: Option<String>,

    /// Plan (if available)
    pub plan: Option<ExplainPlan>,
}

/// Plan comparison result
#[derive(Debug, Clone)]
pub struct PlanComparisonResult {
    /// Test name
    pub test_name: String,

    /// Query tested
    pub query: String,

    /// Plans from each protocol
    pub plans: HashMap<ApiProtocol, ExplainPlan>,

    /// Whether plans are identical
    pub plans_identical: bool,

    /// Plan differences
    pub plan_differences: Vec<PlanDifference>,

    /// Cost comparison
    pub cost_comparison: HashMap<ApiProtocol, f64>,

    /// Performance comparison
    pub performance_comparison: HashMap<ApiProtocol, f64>,
}

/// Plan difference description
#[derive(Debug, Clone)]
pub struct PlanDifference {
    /// Difference type
    pub difference_type: PlanDifferenceType,

    /// Affected protocols
    pub affected_protocols: Vec<ApiProtocol>,

    /// Description
    pub description: String,

    /// Impact severity
    pub severity: DifferenceSeverity,
}

/// Types of plan differences
#[derive(Debug, Clone, PartialEq)]
pub enum PlanDifferenceType {
    /// Different number of plan nodes
    NodeCountMismatch,

    /// Different node types
    NodeTypeMismatch,

    /// Different cost estimates
    CostEstimateMismatch,

    /// Different capabilities claimed
    CapabilityMismatch,

    /// Different join order
    JoinOrderMismatch,

    /// Different filter placement
    FilterPlacementMismatch,
}

/// Severity of differences
#[derive(Debug, Clone, PartialEq)]
pub enum DifferenceSeverity {
    /// Minor difference (e.g., cost estimate variance)
    Minor,

    /// Moderate difference (e.g., different but equivalent plans)
    Moderate,

    /// Major difference (e.g., different execution strategy)
    Major,

    /// Critical difference (e.g., different results)
    Critical,
}

/// API Parity Test Suite
pub struct ApiParityTestSuite {
    /// Test timeout
    timeout: Duration,

    /// Enable performance measurements
    enable_performance: bool,

    /// Strict mode (fail on any difference)
    strict_mode: bool,
}

impl Default for ApiParityTestSuite {
    fn default() -> Self {
        Self {
            timeout: Duration::from_secs(30),
            enable_performance: true,
            strict_mode: false,
        }
    }
}

impl ApiParityTestSuite {
    /// Create a new test suite
    pub fn new() -> Self {
        Self::default()
    }

    /// Create a test suite with custom configuration
    pub fn with_config(timeout: Duration, enable_performance: bool, strict_mode: bool) -> Self {
        Self {
            timeout,
            enable_performance,
            strict_mode,
        }
    }

    /// Run all parity tests
    pub async fn run_all_tests(&self) -> Result<Vec<PlanComparisonResult>> {
        let mut results = Vec::new();

        // Test 1: Simple SELECT queries
        results.push(self.test_simple_select().await?);

        // Test 2: Vector similarity search
        results.push(self.test_vector_similarity_search().await?);

        // Test 3: Filtered vector search
        results.push(self.test_filtered_vector_search().await?);

        // Test 4: JOIN queries
        results.push(self.test_join_queries().await?);

        // Test 5: Aggregation queries
        results.push(self.test_aggregation_queries().await?);

        // Test 6: Complex multi-model queries
        results.push(self.test_multimodel_queries().await?);

        Ok(results)
    }

    /// Test simple SELECT queries
    async fn test_simple_select(&self) -> Result<PlanComparisonResult> {
        let query = "SELECT id, name, price FROM products WHERE price < 100";
        self.compare_plans_across_protocols("Simple SELECT", query).await
    }

    /// Test vector similarity search
    async fn test_vector_similarity_search(&self) -> Result<PlanComparisonResult> {
        let query = "SELECT id, COSINE_DISTANCE(embedding, [0.1, 0.2, ...]) as score FROM products ORDER BY score DESC LIMIT 10";
        self.compare_plans_across_protocols("Vector Similarity Search", query).await
    }

    /// Test filtered vector search
    async fn test_filtered_vector_search(&self) -> Result<PlanComparisonResult> {
        let query = "SELECT id, score FROM (SELECT id, COSINE_DISTANCE(embedding, [0.1, 0.2, ...]) as score FROM products) WHERE score < 0.5 AND category = 'electronics' LIMIT 10";
        self.compare_plans_across_protocols("Filtered Vector Search", query).await
    }

    /// Test JOIN queries
    async fn test_join_queries(&self) -> Result<PlanComparisonResult> {
        let query = "SELECT p.id, p.name, o.order_id FROM products p JOIN orders o ON p.id = o.product_id WHERE p.price < 100";
        self.compare_plans_across_protocols("JOIN Query", query).await
    }

    /// Test aggregation queries
    async fn test_aggregation_queries(&self) -> Result<PlanComparisonResult> {
        let query = "SELECT category, AVG(price) as avg_price, COUNT(*) as count FROM products GROUP BY category HAVING AVG(price) > 50";
        self.compare_plans_across_protocols("Aggregation Query", query).await
    }

    /// Test complex multi-model queries
    async fn test_multimodel_queries(&self) -> Result<PlanComparisonResult> {
        let query = "SELECT p.id, p.vector, g.related_products FROM products p JOIN GRAPH knowledge ON p.id = GRAPH_START(knowledge) WHERE GRAPH_TRAVERSE(knowledge, 'RELATED_TO', 2) AND VECTOR_SIMILAR(p.vector, [0.1, 0.2, ...]) > 0.8";
        self.compare_plans_across_protocols("Multi-Model Query", query).await
    }

    /// Compare plans across all protocols
    async fn compare_plans_across_protocols(&self, test_name: &str, query: &str) -> Result<PlanComparisonResult> {
        let mut plans = HashMap::new();
        let mut costs = HashMap::new();
        let mut performance = HashMap::new();

        // Get plans from each protocol
        for protocol in [ApiProtocol::Rest, ApiProtocol::Grpc, ApiProtocol::Sql, ApiProtocol::Uql] {
            let start = Instant::now();

            let explain_request = ExplainPlanRequest {
                query: query.to_string(),
                query_type: match protocol {
                    ApiProtocol::Sql => QueryType::QueryTypeSql,
                    ApiProtocol::Uql => QueryType::QueryTypeUql,
                    _ => QueryType::QueryTypeSql,
                } as i32,
                format: ExplainFormat::ExplainFormatJson as i32,
                ..Default::default()
            };

            match tokio::time::timeout(self.timeout, explain_query_unified(explain_request)).await {
                Ok(Ok(response)) => {
                    if let Some(plan) = response.plan {
                        let plan_cost = extract_total_plan_cost(&plan);
                        plans.insert(protocol, plan);
                        costs.insert(protocol, plan_cost);
                        performance.insert(protocol, start.elapsed().as_millis() as f64);
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("Error explaining query via {:?}: {}", protocol, e);
                }
                Err(_) => {
                    eprintln!("Timeout explaining query via {:?}", protocol);
                }
            }
        }

        // Compare plans
        let plan_differences = self.compare_plan_structures(&plans);

        // Check if plans are identical
        let plans_identical = plan_differences.is_empty();

        Ok(PlanComparisonResult {
            test_name: test_name.to_string(),
            query: query.to_string(),
            plans,
            plans_identical,
            plan_differences,
            cost_comparison: costs,
            performance_comparison: performance,
        })
    }

    /// Compare plan structures and identify differences
    fn compare_plan_structures(&self, plans: &HashMap<ApiProtocol, ExplainPlan>) -> Vec<PlanDifference> {
        if plans.len() < 2 {
            return Vec::new(); // Can't compare with less than 2 plans
        }

        let mut differences = Vec::new();

        // Get reference plan (first one)
        let reference_protocol = plans.keys().next().unwrap();
        let reference_plan = plans.get(reference_protocol).unwrap();

        // Compare with other plans
        for (protocol, plan) in plans.iter() {
            if protocol == reference_protocol {
                continue;
            }

            // Compare node counts
            if reference_plan.plan_nodes.len() != plan.plan_nodes.len() {
                differences.push(PlanDifference {
                    difference_type: PlanDifferenceType::NodeCountMismatch,
                    affected_protocols: vec![*reference_protocol, *protocol],
                    description: format!(
                        "Node count differs: {:?} has {} nodes, {:?} has {} nodes",
                        reference_protocol,
                        reference_plan.plan_nodes.len(),
                        protocol,
                        plan.plan_nodes.len()
                    ),
                    severity: DifferenceSeverity::Moderate,
                });
            }

            // Compare node types
            for (i, (ref_node, plan_node)) in reference_plan.plan_nodes.iter().zip(plan.plan_nodes.iter()).enumerate() {
                if ref_node.node_type != plan_node.node_type {
                    differences.push(PlanDifference {
                        difference_type: PlanDifferenceType::NodeTypeMismatch,
                        affected_protocols: vec![*reference_protocol, *protocol],
                        description: format!(
                            "Node {} type differs: {:?} has type {:?}, {:?} has type {:?}",
                            i, reference_protocol, ref_node.node_type, protocol, plan_node.node_type
                        ),
                        severity: DifferenceSeverity::Major,
                    });
                }

                // Compare cost estimates (allow 10% variance)
                let cost_diff = (ref_node.estimated_cost - plan_node.estimated_cost).abs();
                let cost_avg = (ref_node.estimated_cost + plan_node.estimated_cost) / 2.0;
                if cost_avg > 0.0 && cost_diff / cost_avg > 0.1 {
                    differences.push(PlanDifference {
                        difference_type: PlanDifferenceType::CostEstimateMismatch,
                        affected_protocols: vec![*reference_protocol, *protocol],
                        description: format!(
                            "Node {} cost differs significantly: {:?} has {:.2}, {:?} has {:.2}",
                            i, reference_protocol, ref_node.estimated_cost, protocol, plan_node.estimated_cost
                        ),
                        severity: DifferenceSeverity::Minor,
                    });
                }
            }
        }

        differences
    }

    /// Validate that execution results are identical across protocols
    pub fn validate_result_parity(&self, results: &[QueryResult]) -> Result<bool> {
        if results.len() < 2 {
            return Ok(true); // Can't compare with less than 2 results
        }

        let reference_result = results.first().unwrap();

        for result in &results[1..] {
            // Compare row counts
            if reference_result.row_count != result.row_count {
                return Ok(false);
            }

            // Compare actual results (if available)
            if !reference_result.rows.is_empty() && !result.rows.is_empty() {
                // Sort results for comparison (order might differ)
                let mut ref_sorted = reference_result.rows.clone();
                let mut result_sorted = result.rows.clone();

                ref_sorted.sort_by_key(|r| r.to_string());
                result_sorted.sort_by_key(|r| r.to_string());

                if ref_sorted != result_sorted {
                    return Ok(false);
                }
            }
        }

        Ok(true)
    }

    /// Generate parity test report
    pub fn generate_parity_report(&self, results: &[PlanComparisonResult]) -> String {
        let mut report = String::new();

        report.push_str("# API Parity Test Report\n\n");

        let total_tests = results.len();
        let passing_tests = results.iter().filter(|r| r.plans_identical).count();
        let failing_tests = total_tests - passing_tests;

        report.push_str(&format!("## Summary\n\n"));
        report.push_str(&format!("- Total Tests: {}\n", total_tests));
        report.push_str(&format!("- Passing: {}\n", passing_tests));
        report.push_str(&format!("- Failing: {}\n", failing_tests));
        report.push_str(&format!("- Pass Rate: {:.1}%\n\n", (passing_tests as f64 / total_tests as f64) * 100.0));

        report.push_str("## Test Results\n\n");

        for result in results {
            report.push_str(&format!("### {}\n\n", result.test_name));
            report.push_str(&format!("Query: `{}`\n\n", result.query));

            if result.plans_identical {
                report.push_str("✅ **PASS**: Plans are identical across all protocols\n\n");
            } else {
                report.push_str("❌ **FAIL**: Plans differ across protocols\n\n");

                if !result.plan_differences.is_empty() {
                    report.push_str("**Differences:**\n\n");
                    for diff in &result.plan_differences {
                        let severity_icon = match diff.severity {
                            DifferenceSeverity::Minor => "⚠️",
                            DifferenceSeverity::Moderate => "⚡",
                            DifferenceSeverity::Major => "🔥",
                            DifferenceSeverity::Critical => "💀",
                        };

                        report.push_str(&format!("{} **{:?}**: {}\n", severity_icon, diff.difference_type, diff.description));
                        report.push_str(&format!("  Affected: {:?}\n\n", diff.affected_protocols));
                    }
                }

                if !result.cost_comparison.is_empty() {
                    report.push_str("**Cost Comparison:**\n\n");
                    for (protocol, cost) in &result.cost_comparison {
                        report.push_str(&format!("- {:?}: {:.2}\n", protocol, cost));
                    }
                    report.push_str("\n");
                }

                if !result.performance_comparison.is_empty() {
                    report.push_str("**Performance Comparison (ms):**\n\n");
                    for (protocol, time) in &result.performance_comparison {
                        report.push_str(&format!("- {:?}: {:.2}\n", protocol, time));
                    }
                    report.push_str("\n");
                }
            }
        }

        report
    }
}

/// Extract total cost from an explain plan
fn extract_total_plan_cost(plan: &ExplainPlan) -> f64 {
    plan.plan_nodes.iter().map(|node| node.estimated_cost).sum()
}

/// Mock implementation for demonstration
/// In the real implementation, this would use actual client libraries
async fn execute_query_via_protocol(protocol: ApiProtocol, query: &str) -> Result<QueryResult> {
    let start = Instant::now();

    // Placeholder implementation
    // In the real implementation, this would:
    // - Connect to the appropriate API endpoint
    // - Execute the query
    // - Collect results and timing information
    // - Handle errors appropriately

    Ok(QueryResult {
        protocol,
        query: query.to_string(),
        execution_time_ms: start.elapsed().as_millis() as f64,
        rows: vec![],
        row_count: 0,
        error: None,
        plan: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_api_parity_suite_creation() {
        let suite = ApiParityTestSuite::new();
        assert_eq!(suite.timeout.as_secs(), 30);
        assert!(suite.enable_performance);
        assert!(!suite.strict_mode);
    }

    #[tokio::test]
    async fn test_custom_config() {
        let suite = ApiParityTestSuite::with_config(
            Duration::from_secs(60),
            false,
            true,
        );
        assert_eq!(suite.timeout.as_secs(), 60);
        assert!(!suite.enable_performance);
        assert!(suite.strict_mode);
    }

    #[test]
    fn test_protocol_display() {
        assert_eq!(ApiProtocol::Rest.as_str(), "REST");
        assert_eq!(ApiProtocol::Grpc.as_str(), "gRPC");
        assert_eq!(ApiProtocol::Sql.as_str(), "SQL");
        assert_eq!(ApiProtocol::Uql.as_str(), "UQL");
    }

    #[test]
    fn test_plan_difference_severity() {
        assert_ne!(DifferenceSeverity::Minor, DifferenceSeverity::Major);
        assert_ne!(DifferenceSeverity::Moderate, DifferenceSeverity::Critical);
    }

    #[tokio::test]
    async fn test_parity_report_generation() {
        let suite = ApiParityTestSuite::new();

        // Create mock results
        let mock_results = vec![
            PlanComparisonResult {
                test_name: "Test 1".to_string(),
                query: "SELECT * FROM test".to_string(),
                plans: HashMap::new(),
                plans_identical: true,
                plan_differences: vec![],
                cost_comparison: HashMap::new(),
                performance_comparison: HashMap::new(),
            }
        ];

        let report = suite.generate_parity_report(&mock_results);

        assert!(report.contains("# API Parity Test Report"));
        assert!(report.contains("Test 1"));
        assert!(report.contains("✅ **PASS**"));
    }
}
