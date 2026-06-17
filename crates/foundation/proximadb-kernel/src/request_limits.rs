//! Limits, timeouts, and work budgets shared across graph/vector queries.

/// Limits and budgets for query execution
#[derive(Debug, Clone, Default)]
pub struct RequestLimits {
    /// Maximum execution time in milliseconds
    pub timeout_ms: Option<u64>,
    /// Maximum computational work budget
    pub work_budget: Option<u64>,
    /// Maximum frontier size for graph traversals
    pub max_frontier: Option<usize>,
    /// Maximum number of results to return
    pub max_results: Option<usize>,
}
