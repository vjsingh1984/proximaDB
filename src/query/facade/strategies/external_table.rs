//! # External Table Strategy
//!
//! Compile-safe facade strategy for SQL that references external tables.
//!
//! The strategy currently performs lightweight detection only. Execution is
//! intentionally explicit: external-table scans are recognized, then rejected
//! with a clear "not wired" error instead of silently falling through to the
//! wrong strategy or returning fabricated results.

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use tracing::{info, instrument};

use crate::catalog::CatalogManager;
use crate::query::facade::{
    ExecutionMetrics, QueryContent, QueryContext, QueryRequest, QueryResult, QueryResultData,
    QueryStrategy, QueryType,
};

/// External table strategy for SQL that references catalog-backed tables.
pub struct ExternalTableStrategy {
    catalog_manager: Arc<CatalogManager>,
    priority: i32,
}

impl ExternalTableStrategy {
    /// Create a new strategy.
    pub fn new(catalog_manager: Arc<CatalogManager>) -> Self {
        Self {
            catalog_manager,
            priority: 70,
        }
    }

    /// Override strategy priority.
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    fn extract_sql(&self, request: &QueryRequest) -> Result<&str> {
        match &request.content {
            QueryContent::Sql(query) => Ok(query.as_str()),
            _ => Err(anyhow!("ExternalTableStrategy requires SQL content")),
        }
    }

    fn has_external_tables(&self, sql: &str) -> bool {
        let normalized = sql.to_ascii_lowercase();

        normalized.contains("iceberg.")
            || normalized.contains("delta.")
            || normalized.contains("glue.")
            || normalized.contains("parquet.")
            || normalized.contains("external.")
    }
}

#[async_trait]
impl QueryStrategy for ExternalTableStrategy {
    fn name(&self) -> &str {
        "external_table"
    }

    fn can_handle(&self, request: &QueryRequest) -> bool {
        if request.query_type != QueryType::Sql && request.query_type != QueryType::Federated {
            return false;
        }

        let Ok(sql) = self.extract_sql(request) else {
            return false;
        };

        self.has_external_tables(sql)
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    #[instrument(skip(self, request, _ctx))]
    async fn execute(&self, request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        let start = Instant::now();
        let sql = self.extract_sql(&request)?;

        info!("External table query detected by facade");

        let _ = &self.catalog_manager;
        let _metrics = ExecutionMetrics {
            execution_path: "external_table".to_string(),
            strategy_name: self.name().to_string(),
            execution_time_ms: start.elapsed().as_millis() as u64,
            planning_time_ms: 0,
            results_scanned: 0,
            results_returned: 0,
            cache_hit: false,
            extra: serde_json::json!({ "sql": sql }),
        };

        let _unused_result = QueryResult {
            data: QueryResultData::Rows(Vec::new()),
            metrics: None,
        };

        Err(anyhow!(
            "External table queries are detected but not wired into live execution yet"
        ))
    }
}

/// Placeholder scanner surface for future external table execution.
pub struct ExternalTableScanner {
    catalog_manager: Arc<CatalogManager>,
}

impl ExternalTableScanner {
    pub fn new(catalog_manager: Arc<CatalogManager>) -> Self {
        Self { catalog_manager }
    }

    pub async fn scan_table(
        &self,
        _catalog: &str,
        _namespace: &[String],
        _table: &str,
        _projections: Vec<String>,
        _predicates: Vec<String>,
    ) -> Result<Vec<RecordBatch>> {
        let _ = &self.catalog_manager;
        Err(anyhow!(
            "External table scanning is not wired into live execution yet"
        ))
    }

    pub async fn get_table_stats(
        &self,
        _catalog: &str,
        _namespace: &[String],
        _table: &str,
    ) -> Result<TableStatistics> {
        let _ = &self.catalog_manager;
        Err(anyhow!(
            "External table statistics are not wired into live execution yet"
        ))
    }
}

#[derive(Debug, Clone, Default)]
pub struct TableStatistics {
    pub row_count: u64,
    pub num_files: u64,
    pub size_bytes: u64,
}

/// Placeholder predicate pushdown surface for future execution.
pub struct ExternalPredicatePushdown {
    catalog_manager: Arc<CatalogManager>,
}

impl ExternalPredicatePushdown {
    pub fn new(catalog_manager: Arc<CatalogManager>) -> Self {
        Self { catalog_manager }
    }

    pub fn pushdown_predicates(
        &self,
        _catalog: &str,
        _namespace: &[String],
        _table: &str,
        _predicates: &[String],
    ) -> Result<Vec<PushedPredicate>> {
        let _ = &self.catalog_manager;
        Ok(Vec::new())
    }
}

#[derive(Debug, Clone)]
pub struct PushedPredicate {
    pub column: String,
    pub expression: String,
    pub partition_prunable: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_external_tables() {
        let strategy = ExternalTableStrategy::new(Arc::new(CatalogManager::new()));

        assert!(strategy.has_external_tables("SELECT * FROM iceberg.db.table"));
        assert!(strategy.has_external_tables("SELECT * FROM delta.db.table"));
        assert!(strategy.has_external_tables("SELECT * FROM glue.db.table"));
        assert!(!strategy.has_external_tables("SELECT * FROM local_table"));
    }

    #[test]
    fn test_priority() {
        let strategy = ExternalTableStrategy::new(Arc::new(CatalogManager::new()));
        assert_eq!(strategy.priority(), 70);
    }
}
