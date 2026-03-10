//! Request limits, timeouts, and work budgets shared by graph/vector queries.

#[derive(Debug, Clone, Default)]
pub struct RequestLimits {
    pub timeout_ms: Option<u64>,
    pub work_budget: Option<u64>,
    pub max_frontier: Option<usize>, // for traversals
    pub max_results: Option<usize>,
}
