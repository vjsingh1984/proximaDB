//! Multi-model SQL executor - SqlPlan lowering + dispatch
//!
//! This module handles lowering SQL plans to execution and dispatching to appropriate engines.

// TODO: Implement SQL plan lowering and dispatch logic

#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// Maximum parallelism for query execution
    pub max_parallelism: usize,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self { max_parallelism: 4 }
    }
}

#[derive(Debug, Clone)]
pub struct Executor;

impl Executor {
    pub fn new(_config: ExecutorConfig) -> Self {
        Self
    }

    pub fn execute(
        &self,
        _plan: &crate::query::multimodel_executor::SqlPlan,
    ) -> anyhow::Result<crate::services::search::ResultBatch> {
        // TODO: Implement actual execution
        anyhow::bail!("Executor::execute not yet implemented")
    }
}
