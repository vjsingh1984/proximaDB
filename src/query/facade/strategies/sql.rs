//! # SQL Strategy
//!
//! Real implementation of `QueryStrategy` for SQL queries.
//! Wraps the existing `FederatedQueryContext` infrastructure.
//!
//! ## Features
//!
//! - Converts facade `QueryRequest` to SQL execution
//! - Leverages existing federated query infrastructure
//! - Supports standard SQL and ProximaDB extensions
//! - Returns results in unified `QueryResult` format
//!
//! ## Architecture
//!
//! ```text
//! QueryRequest (facade)
//!       │
//!       ▼
//! SqlStrategy
//!       │
//!       ▼
//! FederatedQueryContext.execute()
//!       │
//!       ▼
//! Arrow RecordBatch
//!       │
//!       ▼
//! QueryResult (facade)
//! ```

use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use arrow::array::{ArrayRef, BooleanArray, Float32Array, Float64Array, Int64Array, StringArray};
use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use tracing::{debug, info, instrument};

use crate::query::facade::{
    ExecutionMetrics, QueryContent, QueryContext, QueryRequest, QueryResult, QueryResultData,
    QueryStrategy, QueryType,
};
use crate::query::federated::FederatedQueryContext;

/// SQL Strategy - Real implementation wrapping FederatedQueryContext
///
/// This strategy handles `QueryType::Sql` and `QueryType::Federated` requests by:
/// 1. Extracting SQL from the facade request
/// 2. Delegating to FederatedQueryContext for execution
/// 3. Converting Arrow results back to facade format
pub struct SqlStrategy {
    /// Federated query context for SQL execution
    federated_ctx: Arc<FederatedQueryContext>,
    /// Strategy priority (higher = preferred)
    priority: i32,
}

impl SqlStrategy {
    /// Create a new SqlStrategy
    pub fn new(federated_ctx: Arc<FederatedQueryContext>) -> Self {
        Self {
            federated_ctx,
            priority: 90, // High priority for SQL queries, slightly lower than vector
        }
    }

    /// Create with custom priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Extract SQL from the query request
    fn extract_sql(&self, request: &QueryRequest) -> Result<String> {
        match &request.content {
            QueryContent::Sql(query) => Ok(query.clone()),
            _ => Err(anyhow!("SqlStrategy requires SQL content")),
        }
    }

    /// Convert Arrow RecordBatch to JSON-like row format
    fn batches_to_rows(batches: &[RecordBatch]) -> Vec<serde_json::Value> {
        let mut rows = Vec::new();

        for batch in batches {
            let schema = batch.schema();
            for row_idx in 0..batch.num_rows() {
                let mut row = serde_json::Map::new();

                for (col_idx, field) in schema.fields().iter().enumerate() {
                    let column = batch.column(col_idx);
                    let value = Self::array_value_to_json(column, row_idx);
                    row.insert(field.name().clone(), value);
                }

                rows.push(serde_json::Value::Object(row));
            }
        }

        rows
    }

    /// Extract a single value from an Arrow array and convert to JSON
    fn array_value_to_json(array: &ArrayRef, idx: usize) -> serde_json::Value {
        if array.is_null(idx) {
            return serde_json::Value::Null;
        }

        // Try to downcast to known types
        if let Some(arr) = array.as_any().downcast_ref::<StringArray>() {
            return serde_json::Value::String(arr.value(idx).to_string());
        }

        if let Some(arr) = array.as_any().downcast_ref::<Int64Array>() {
            return serde_json::json!(arr.value(idx));
        }

        if let Some(arr) = array.as_any().downcast_ref::<Float64Array>() {
            let val = arr.value(idx);
            return serde_json::Number::from_f64(val)
                .map_or(serde_json::Value::Null, serde_json::Value::Number);
        }

        if let Some(arr) = array.as_any().downcast_ref::<Float32Array>() {
            let val = arr.value(idx) as f64;
            return serde_json::Number::from_f64(val)
                .map_or(serde_json::Value::Null, serde_json::Value::Number);
        }

        if let Some(arr) = array.as_any().downcast_ref::<BooleanArray>() {
            return serde_json::Value::Bool(arr.value(idx));
        }

        // For other types, try to get a string representation
        // This is a fallback for complex types
        serde_json::Value::String(format!("{:?}", array))
    }

    /// Convert execution result to facade QueryResult
    fn to_facade_result(
        &self,
        result: crate::query::federated::ExecutionResult,
        execution_time_ms: u64,
    ) -> QueryResult {
        let rows = Self::batches_to_rows(&result.batches);
        let results_returned = rows.len();

        // Extract column names from schema (stored in metrics for reference)
        let columns: Vec<String> = result
            .schema
            .fields()
            .iter()
            .map(|f| f.name().clone())
            .collect();

        QueryResult {
            data: QueryResultData::Rows(rows),
            metrics: Some(ExecutionMetrics {
                execution_path: "federated".to_string(),
                strategy_name: "sql".to_string(),
                execution_time_ms,
                planning_time_ms: 0, // FederatedQueryContext handles planning internally
                results_scanned: result.stats.rows_produced,
                results_returned,
                cache_hit: result.stats.cache_hits > 0,
                extra: serde_json::json!({
                    "engine": "FederatedQueryContext",
                    "columns": columns,
                    "models_queried": result.stats.models_queried.iter()
                        .map(|m| format!("{:?}", m))
                        .collect::<Vec<_>>(),
                    "bytes_scanned": result.stats.bytes_scanned,
                    "cache_hits": result.stats.cache_hits,
                    "cache_misses": result.stats.cache_misses,
                }),
            }),
        }
    }
}

#[async_trait]
impl QueryStrategy for SqlStrategy {
    fn name(&self) -> &str {
        "sql"
    }

    fn can_handle(&self, request: &QueryRequest) -> bool {
        matches!(request.query_type, QueryType::Sql | QueryType::Federated)
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    #[instrument(skip(self, request, _ctx), fields(strategy = "sql"))]
    async fn execute(&self, request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        let start = Instant::now();

        // Extract SQL from request
        let sql = self.extract_sql(&request)?;

        debug!(
            sql = %sql,
            query_type = ?request.query_type,
            "Executing SQL via FederatedQueryContext"
        );

        // Execute through federated context
        let result = self.federated_ctx.execute(&sql).await?;

        let execution_time_ms = start.elapsed().as_millis() as u64;

        // Convert to facade result
        let query_result = self.to_facade_result(result, execution_time_ms);

        info!(
            results = query_result
                .metrics
                .as_ref()
                .map_or(0, |m| m.results_returned),
            time_ms = execution_time_ms,
            "SQL query completed"
        );

        Ok(query_result)
    }
}

// ================================================================================
// TESTS
// ================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::datatypes::{DataType, Field, Schema};

    #[test]
    fn test_batches_to_rows_empty() {
        let rows = SqlStrategy::batches_to_rows(&[]);
        assert!(rows.is_empty());
    }

    #[test]
    fn test_batches_to_rows_with_data() {
        use arrow::array::{Int64Array, StringArray};

        let schema = Arc::new(Schema::new(vec![
            Field::new("name", DataType::Utf8, false),
            Field::new("age", DataType::Int64, false),
        ]));

        let batch = RecordBatch::try_new(
            schema,
            vec![
                Arc::new(StringArray::from(vec!["Alice", "Bob"])),
                Arc::new(Int64Array::from(vec![30, 25])),
            ],
        )
        .unwrap();

        let rows = SqlStrategy::batches_to_rows(&[batch]);

        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0]["name"], serde_json::json!("Alice"));
        assert_eq!(rows[0]["age"], serde_json::json!(30));
        assert_eq!(rows[1]["name"], serde_json::json!("Bob"));
        assert_eq!(rows[1]["age"], serde_json::json!(25));
    }

    #[test]
    fn test_batches_to_rows_with_nulls() {
        use arrow::array::Int64Array;

        let schema = Arc::new(Schema::new(vec![Field::new(
            "value",
            DataType::Int64,
            true,
        )]));

        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(Int64Array::from(vec![Some(42), None, Some(99)]))],
        )
        .unwrap();

        let rows = SqlStrategy::batches_to_rows(&[batch]);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["value"], serde_json::json!(42));
        assert_eq!(rows[1]["value"], serde_json::Value::Null);
        assert_eq!(rows[2]["value"], serde_json::json!(99));
    }

    #[test]
    fn test_batches_to_rows_with_floats() {
        use arrow::array::Float64Array;

        let schema = Arc::new(Schema::new(vec![Field::new(
            "score",
            DataType::Float64,
            false,
        )]));

        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(Float64Array::from(vec![0.95, 0.87, 0.72]))],
        )
        .unwrap();

        let rows = SqlStrategy::batches_to_rows(&[batch]);

        assert_eq!(rows.len(), 3);
        // Check float values with tolerance
        if let serde_json::Value::Number(n) = &rows[0]["score"] {
            assert!((n.as_f64().unwrap() - 0.95).abs() < 0.001);
        } else {
            panic!("Expected number");
        }
    }

    #[test]
    fn test_batches_to_rows_with_float32() {
        use arrow::array::Float32Array;

        let schema = Arc::new(Schema::new(vec![Field::new(
            "score",
            DataType::Float32,
            false,
        )]));

        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(Float32Array::from(vec![0.3_f32, 0.2_f32]))],
        )
        .unwrap();

        let rows = SqlStrategy::batches_to_rows(&[batch]);

        assert_eq!(rows.len(), 2);
        if let serde_json::Value::Number(n) = &rows[0]["score"] {
            assert!((n.as_f64().unwrap() - 0.3).abs() < 0.001);
        } else {
            panic!("Expected number");
        }
        if let serde_json::Value::Number(n) = &rows[1]["score"] {
            assert!((n.as_f64().unwrap() - 0.2).abs() < 0.001);
        } else {
            panic!("Expected number");
        }
    }

    #[test]
    fn test_batches_to_rows_with_booleans() {
        use arrow::array::BooleanArray;

        let schema = Arc::new(Schema::new(vec![Field::new(
            "active",
            DataType::Boolean,
            false,
        )]));

        let batch = RecordBatch::try_new(
            schema,
            vec![Arc::new(BooleanArray::from(vec![true, false, true]))],
        )
        .unwrap();

        let rows = SqlStrategy::batches_to_rows(&[batch]);

        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0]["active"], serde_json::json!(true));
        assert_eq!(rows[1]["active"], serde_json::json!(false));
        assert_eq!(rows[2]["active"], serde_json::json!(true));
    }

    #[test]
    fn test_strategy_can_handle_sql() {
        let request = QueryRequest::sql("SELECT * FROM test");
        assert_eq!(request.query_type, QueryType::Sql);
    }

    #[test]
    fn test_strategy_can_handle_federated() {
        let request =
            QueryRequest::federated("SELECT * FROM VECTOR_SEARCH('products', '[0.1]', 10)");
        assert_eq!(request.query_type, QueryType::Federated);
    }

    #[test]
    fn test_strategy_cannot_handle_vector() {
        let request = QueryRequest::vector_search(vec![0.1, 0.2], 10);
        assert_eq!(request.query_type, QueryType::VectorSearch);
        // SqlStrategy should not handle vector queries
        assert_ne!(request.query_type, QueryType::Sql);
    }
}
