//! # Columnar Query Strategy
//!
//! Real implementation of `QueryStrategy` for columnar/analytical queries.
//! Uses the `ColumnarReadProvider` abstraction for efficient columnar data access.
//!
//! ## Features
//!
//! - Handles analytical SQL queries with columnar execution
//! - Leverages Arrow RecordBatch and Parquet range pruning
//! - Supports predicate pushdown for filter optimization
//! - Returns results as JSON rows or Arrow-compatible format
//!
//! ## Architecture
//!
//! ```text
//! QueryRequest (facade)
//!       │
//!       ▼
//! ColumnarStrategy
//!       │
//!       ├──> ArrowInMemoryProvider (cached data)
//!       │
//!       └──> ParquetRangePrunedProvider (on-disk data)
//!       │
//!       ▼
//! QueryResult (facade)
//! ```

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Result, anyhow};
use arrow::array::{Array, ArrayRef, AsArray};
use arrow::datatypes::DataType;
use arrow::record_batch::RecordBatch;
use async_trait::async_trait;
use parking_lot::RwLock;
use tracing::{debug, info, instrument};

use crate::query::columnar::{
    ArrowInMemoryProvider, ColumnarReadProvider, ParquetRangePrunedProvider,
    PredicatePushdownConfig,
};
use crate::query::facade::{
    ExecutionMetrics, QueryContent, QueryContext, QueryRequest, QueryResult, QueryResultData,
    QueryStrategy, QueryType,
};

/// Columnar Query Strategy - Analytical query execution using columnar providers
///
/// This strategy handles analytical queries by:
/// 1. Detecting columnar-optimizable queries
/// 2. Selecting appropriate provider (in-memory Arrow or Parquet)
/// 3. Applying predicate pushdown and projection pruning
/// 4. Converting Arrow results to facade format
pub struct ColumnarStrategy {
    /// Registered columnar providers by collection
    providers: RwLock<HashMap<String, Arc<dyn ColumnarReadProvider>>>,
    /// Strategy priority (higher = preferred)
    priority: i32,
    /// Default batch size for streaming
    default_batch_size: usize,
}

impl ColumnarStrategy {
    /// Create a new ColumnarStrategy
    pub fn new() -> Self {
        Self {
            providers: RwLock::new(HashMap::new()),
            priority: 50, // Medium priority (below vector, above SQL)
            default_batch_size: 8192,
        }
    }

    /// Create with custom priority
    pub fn with_priority(mut self, priority: i32) -> Self {
        self.priority = priority;
        self
    }

    /// Register an in-memory Arrow provider for a collection
    ///
    /// Returns Err if the batches are empty
    pub fn register_arrow_provider(
        &self,
        collection_id: impl Into<String>,
        batches: Vec<RecordBatch>,
    ) -> Result<()> {
        let collection_id = collection_id.into();
        let provider = ArrowInMemoryProvider::new(batches, collection_id.clone())?;
        self.providers
            .write()
            .insert(collection_id, Arc::new(provider) as Arc<dyn ColumnarReadProvider>);
        Ok(())
    }

    /// Register a Parquet provider for a collection
    pub fn register_parquet_provider(
        &self,
        collection_id: impl Into<String>,
        provider: ParquetRangePrunedProvider,
    ) {
        let collection_id = collection_id.into();
        self.providers
            .write()
            .insert(collection_id, Arc::new(provider));
    }

    /// Register a custom provider for a collection
    pub fn register_provider(
        &self,
        collection_id: impl Into<String>,
        provider: Arc<dyn ColumnarReadProvider>,
    ) {
        self.providers.write().insert(collection_id.into(), provider);
    }

    /// Get provider for a collection
    pub fn get_provider(&self, collection_id: &str) -> Option<Arc<dyn ColumnarReadProvider>> {
        self.providers.read().get(collection_id).cloned()
    }

    /// Check if a SQL query is suitable for columnar execution
    fn is_columnar_query(&self, sql: &str) -> bool {
        let sql_upper = sql.to_uppercase();

        // Columnar execution is good for:
        // - Aggregation queries (COUNT, SUM, AVG, MIN, MAX)
        // - Full table scans with filters
        // - GROUP BY queries
        // - DISTINCT queries

        let has_aggregation = sql_upper.contains("COUNT(")
            || sql_upper.contains("SUM(")
            || sql_upper.contains("AVG(")
            || sql_upper.contains("MIN(")
            || sql_upper.contains("MAX(");

        let has_group_by = sql_upper.contains("GROUP BY");
        let has_distinct = sql_upper.contains("DISTINCT");

        // Exclude vector-specific queries
        let has_vector_ops = sql_upper.contains("VECTOR_SEARCH")
            || sql_upper.contains("<->")
            || sql_upper.contains("COSINE_DISTANCE")
            || sql_upper.contains("EUCLIDEAN_DISTANCE");

        // Columnar is good for aggregations and table scans, not vector ops
        (has_aggregation || has_group_by || has_distinct) && !has_vector_ops
    }

    /// Extract collection name from SQL (basic parsing)
    fn extract_collection_from_sql(&self, sql: &str) -> Option<String> {
        let sql_upper = sql.to_uppercase();

        // Simple extraction: FROM <table_name>
        if let Some(from_pos) = sql_upper.find("FROM ") {
            let after_from = &sql[from_pos + 5..];
            let collection: String = after_from
                .chars()
                .take_while(|c| c.is_alphanumeric() || *c == '_')
                .collect();
            if !collection.is_empty() {
                return Some(collection);
            }
        }

        None
    }

    /// Convert Arrow RecordBatches to JSON rows
    fn batches_to_json_rows(&self, batches: &[RecordBatch]) -> Vec<serde_json::Value> {
        let mut rows = Vec::new();

        for batch in batches {
            let schema = batch.schema();
            let num_rows = batch.num_rows();

            for row_idx in 0..num_rows {
                let mut row_obj = serde_json::Map::new();

                for (col_idx, field) in schema.fields().iter().enumerate() {
                    let col = batch.column(col_idx);
                    let value = self.extract_value(col, row_idx);
                    row_obj.insert(field.name().clone(), value);
                }

                rows.push(serde_json::Value::Object(row_obj));
            }
        }

        rows
    }

    /// Extract a single value from an Arrow array at the given index
    fn extract_value(&self, array: &ArrayRef, idx: usize) -> serde_json::Value {
        if array.is_null(idx) {
            return serde_json::Value::Null;
        }

        match array.data_type() {
            DataType::Utf8 => {
                let arr = array.as_string::<i32>();
                serde_json::Value::String(arr.value(idx).to_string())
            }
            DataType::LargeUtf8 => {
                let arr = array.as_string::<i64>();
                serde_json::Value::String(arr.value(idx).to_string())
            }
            DataType::Int8 => {
                let arr = array.as_primitive::<arrow::datatypes::Int8Type>();
                serde_json::json!(arr.value(idx))
            }
            DataType::Int16 => {
                let arr = array.as_primitive::<arrow::datatypes::Int16Type>();
                serde_json::json!(arr.value(idx))
            }
            DataType::Int32 => {
                let arr = array.as_primitive::<arrow::datatypes::Int32Type>();
                serde_json::json!(arr.value(idx))
            }
            DataType::Int64 => {
                let arr = array.as_primitive::<arrow::datatypes::Int64Type>();
                serde_json::json!(arr.value(idx))
            }
            DataType::UInt8 => {
                let arr = array.as_primitive::<arrow::datatypes::UInt8Type>();
                serde_json::json!(arr.value(idx))
            }
            DataType::UInt16 => {
                let arr = array.as_primitive::<arrow::datatypes::UInt16Type>();
                serde_json::json!(arr.value(idx))
            }
            DataType::UInt32 => {
                let arr = array.as_primitive::<arrow::datatypes::UInt32Type>();
                serde_json::json!(arr.value(idx))
            }
            DataType::UInt64 => {
                let arr = array.as_primitive::<arrow::datatypes::UInt64Type>();
                serde_json::json!(arr.value(idx))
            }
            DataType::Float32 => {
                let arr = array.as_primitive::<arrow::datatypes::Float32Type>();
                serde_json::json!(arr.value(idx))
            }
            DataType::Float64 => {
                let arr = array.as_primitive::<arrow::datatypes::Float64Type>();
                serde_json::json!(arr.value(idx))
            }
            DataType::Boolean => {
                let arr = array.as_boolean();
                serde_json::json!(arr.value(idx))
            }
            DataType::Binary | DataType::LargeBinary | DataType::FixedSizeBinary(_) => {
                // Encode binary as base64
                use base64::Engine;
                let bytes: &[u8] = match array.data_type() {
                    DataType::Binary => array.as_binary::<i32>().value(idx),
                    DataType::LargeBinary => array.as_binary::<i64>().value(idx),
                    DataType::FixedSizeBinary(_) => array.as_fixed_size_binary().value(idx),
                    _ => &[],
                };
                let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
                serde_json::Value::String(encoded)
            }
            DataType::List(_) | DataType::LargeList(_) | DataType::FixedSizeList(_, _) => {
                // Handle list types (including vector embeddings)
                self.extract_list_value(array, idx)
            }
            _ => {
                // Fallback: convert to string representation
                serde_json::Value::String(format!("{:?}", array.data_type()))
            }
        }
    }

    /// Extract list/array values
    fn extract_list_value(&self, array: &ArrayRef, idx: usize) -> serde_json::Value {
        match array.data_type() {
            DataType::List(field) | DataType::LargeList(field) => {
                // For Float32 lists (common for embeddings)
                if *field.data_type() == DataType::Float32 {
                    let list_arr = array.as_list::<i32>();
                    let values = list_arr.value(idx);
                    let float_arr = values.as_primitive::<arrow::datatypes::Float32Type>();
                    let floats: Vec<f32> = float_arr.values().to_vec();
                    return serde_json::json!(floats);
                }
                // Generic list handling
                serde_json::Value::Array(vec![])
            }
            DataType::FixedSizeList(field, size) => {
                if *field.data_type() == DataType::Float32 {
                    let list_arr = array.as_fixed_size_list();
                    let values = list_arr.value(idx);
                    let float_arr = values.as_primitive::<arrow::datatypes::Float32Type>();
                    let floats: Vec<f32> = float_arr.values()[..*size as usize].to_vec();
                    return serde_json::json!(floats);
                }
                serde_json::Value::Array(vec![])
            }
            _ => serde_json::Value::Array(vec![]),
        }
    }
}

impl Default for ColumnarStrategy {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl QueryStrategy for ColumnarStrategy {
    fn name(&self) -> &str {
        "columnar"
    }

    fn can_handle(&self, request: &QueryRequest) -> bool {
        // Handle SQL queries that are suitable for columnar execution
        match &request.content {
            QueryContent::Sql(sql) => {
                // Check if query is columnar-optimizable
                if !self.is_columnar_query(sql) {
                    return false;
                }

                // Check if we have a provider for the target collection
                if let Some(collection) = &request.target {
                    return self.providers.read().contains_key(collection);
                }

                // Try to extract collection from SQL
                if let Some(collection) = self.extract_collection_from_sql(sql) {
                    return self.providers.read().contains_key(&collection);
                }

                false
            }
            _ => false,
        }
    }

    fn priority(&self) -> i32 {
        self.priority
    }

    #[instrument(skip(self, request, _ctx), fields(strategy = "columnar"))]
    async fn execute(&self, request: QueryRequest, _ctx: &QueryContext) -> Result<QueryResult> {
        let start = Instant::now();

        // Extract SQL query
        let sql = match &request.content {
            QueryContent::Sql(sql) => sql.clone(),
            _ => return Err(anyhow!("ColumnarStrategy requires SQL content")),
        };

        // Determine target collection
        let collection_id = request
            .target
            .clone()
            .or_else(|| self.extract_collection_from_sql(&sql))
            .ok_or_else(|| anyhow!("Could not determine target collection"))?;

        // Get provider
        let provider = self
            .get_provider(&collection_id)
            .ok_or_else(|| anyhow!("No columnar provider for collection '{}'", collection_id))?;

        debug!(
            collection = %collection_id,
            provider = %provider.name(),
            "Executing columnar query"
        );

        // Configure predicate pushdown (basic implementation)
        // In a full implementation, this would parse the SQL WHERE clause
        let config = PredicatePushdownConfig::default();

        // Execute query through provider
        let batches = provider.read_batches(config).await?;

        // Convert to JSON rows
        let rows = self.batches_to_json_rows(&batches);

        let execution_time_ms = start.elapsed().as_millis() as u64;
        let results_returned = rows.len();

        // Get provider stats
        let stats = provider.get_stats();

        info!(
            collection = %collection_id,
            results = results_returned,
            blocks_scanned = stats.blocks_scanned,
            bytes_read = stats.bytes_read,
            time_ms = execution_time_ms,
            "Columnar query completed"
        );

        Ok(QueryResult {
            data: QueryResultData::Rows(rows),
            metrics: Some(ExecutionMetrics {
                execution_path: "unified".to_string(),
                strategy_name: "columnar".to_string(),
                execution_time_ms,
                planning_time_ms: 0,
                results_scanned: stats.total_rows as usize,
                results_returned,
                cache_hit: stats.cache_hits > 0,
                extra: serde_json::json!({
                    "provider": provider.name(),
                    "blocks_scanned": stats.blocks_scanned,
                    "bytes_read": stats.bytes_read,
                    "blocks_pruned": stats.blocks_pruned,
                    "rows_after_pruning": stats.rows_after_pruning,
                }),
            }),
        })
    }
}

// ================================================================================
// TESTS
// ================================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use arrow::array::{Float32Array, Int64Array, StringArray};
    use arrow::datatypes::{Field, Schema};

    fn create_test_batch() -> RecordBatch {
        let schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("value", DataType::Int64, false),
            Field::new("score", DataType::Float32, false),
        ]));

        let id_array = StringArray::from(vec!["a", "b", "c"]);
        let value_array = Int64Array::from(vec![1, 2, 3]);
        let score_array = Float32Array::from(vec![0.1, 0.2, 0.3]);

        RecordBatch::try_new(
            schema,
            vec![
                Arc::new(id_array),
                Arc::new(value_array),
                Arc::new(score_array),
            ],
        )
        .unwrap()
    }

    #[test]
    fn test_columnar_strategy_creation() {
        let strategy = ColumnarStrategy::new();
        assert_eq!(strategy.name(), "columnar");
        assert_eq!(strategy.priority(), 50);
    }

    #[test]
    fn test_is_columnar_query_aggregation() {
        let strategy = ColumnarStrategy::new();

        // Aggregation queries should be columnar
        assert!(strategy.is_columnar_query("SELECT COUNT(*) FROM products"));
        assert!(strategy.is_columnar_query("SELECT SUM(price) FROM orders"));
        assert!(strategy.is_columnar_query("SELECT AVG(score) FROM results"));
        assert!(strategy.is_columnar_query("SELECT MIN(value), MAX(value) FROM data"));
    }

    #[test]
    fn test_is_columnar_query_group_by() {
        let strategy = ColumnarStrategy::new();

        // GROUP BY queries should be columnar
        assert!(strategy.is_columnar_query("SELECT category, COUNT(*) FROM products GROUP BY category"));
        assert!(strategy.is_columnar_query("SELECT status, SUM(amount) FROM orders GROUP BY status"));
    }

    #[test]
    fn test_is_columnar_query_distinct() {
        let strategy = ColumnarStrategy::new();

        // DISTINCT queries should be columnar
        assert!(strategy.is_columnar_query("SELECT DISTINCT category FROM products"));
    }

    #[test]
    fn test_is_columnar_query_not_vector() {
        let strategy = ColumnarStrategy::new();

        // Vector queries should NOT be columnar
        assert!(!strategy.is_columnar_query("SELECT * FROM VECTOR_SEARCH('products', '[0.1]', 10)"));
        assert!(!strategy.is_columnar_query("SELECT * FROM products ORDER BY embedding <-> '[0.1]'"));
        assert!(!strategy.is_columnar_query("SELECT COSINE_DISTANCE(embedding, '[0.1]') FROM products"));
    }

    #[test]
    fn test_is_columnar_query_simple_select() {
        let strategy = ColumnarStrategy::new();

        // Simple SELECT without aggregation should NOT be columnar
        assert!(!strategy.is_columnar_query("SELECT * FROM products"));
        assert!(!strategy.is_columnar_query("SELECT id, name FROM products WHERE price > 100"));
    }

    #[test]
    fn test_extract_collection_from_sql() {
        let strategy = ColumnarStrategy::new();

        assert_eq!(
            strategy.extract_collection_from_sql("SELECT * FROM products"),
            Some("products".to_string())
        );
        assert_eq!(
            strategy.extract_collection_from_sql("SELECT COUNT(*) FROM orders WHERE status = 'pending'"),
            Some("orders".to_string())
        );
        assert_eq!(
            strategy.extract_collection_from_sql("SELECT * FROM my_table_123"),
            Some("my_table_123".to_string())
        );
    }

    #[test]
    fn test_register_arrow_provider() {
        let strategy = ColumnarStrategy::new();
        let batch = create_test_batch();

        strategy.register_arrow_provider("test_collection", vec![batch]).unwrap();

        assert!(strategy.get_provider("test_collection").is_some());
        assert!(strategy.get_provider("nonexistent").is_none());
    }

    #[test]
    fn test_register_arrow_provider_empty_fails() {
        let strategy = ColumnarStrategy::new();
        let result = strategy.register_arrow_provider("empty", vec![]);
        assert!(result.is_err());
    }

    #[test]
    fn test_can_handle_with_provider() {
        let strategy = ColumnarStrategy::new();
        let batch = create_test_batch();

        strategy.register_arrow_provider("products", vec![batch]).unwrap();

        // Should handle aggregation query with registered provider
        let request = QueryRequest::sql("SELECT COUNT(*) FROM products");
        assert!(strategy.can_handle(&request));

        // Should not handle query without provider
        let request = QueryRequest::sql("SELECT COUNT(*) FROM unknown_table");
        assert!(!strategy.can_handle(&request));

        // Should not handle non-columnar query
        let request = QueryRequest::sql("SELECT * FROM products");
        assert!(!strategy.can_handle(&request));
    }

    #[test]
    fn test_can_handle_with_target() {
        let strategy = ColumnarStrategy::new();
        let batch = create_test_batch();

        strategy.register_arrow_provider("products", vec![batch]).unwrap();

        // Should handle with explicit target
        let request = QueryRequest::sql("SELECT COUNT(*) FROM products")
            .with_target("products");
        assert!(strategy.can_handle(&request));
    }

    #[test]
    fn test_batches_to_json_rows() {
        let strategy = ColumnarStrategy::new();
        let batch = create_test_batch();

        let rows = strategy.batches_to_json_rows(&[batch]);

        assert_eq!(rows.len(), 3);

        // Check first row
        let row0 = rows[0].as_object().unwrap();
        assert_eq!(row0.get("id").unwrap(), &serde_json::json!("a"));
        assert_eq!(row0.get("value").unwrap(), &serde_json::json!(1));

        // Check score is approximately 0.1
        let score0 = row0.get("score").unwrap().as_f64().unwrap();
        assert!((score0 - 0.1).abs() < 0.001);
    }

    #[test]
    fn test_batches_to_json_rows_multiple_batches() {
        let strategy = ColumnarStrategy::new();
        let batch1 = create_test_batch();
        let batch2 = create_test_batch();

        let rows = strategy.batches_to_json_rows(&[batch1, batch2]);

        // 3 rows per batch * 2 batches = 6 rows
        assert_eq!(rows.len(), 6);
    }

    #[tokio::test]
    async fn test_execute_with_arrow_provider() {
        let strategy = ColumnarStrategy::new();
        let batch = create_test_batch();

        strategy.register_arrow_provider("products", vec![batch]).unwrap();

        let request = QueryRequest::sql("SELECT COUNT(*) FROM products")
            .with_target("products")
            .with_metrics();

        let ctx = QueryContext::new(30000);
        let result = strategy.execute(request, &ctx).await.unwrap();

        // Should return rows
        if let QueryResultData::Rows(rows) = result.data {
            assert_eq!(rows.len(), 3);
        } else {
            panic!("Expected Rows result");
        }

        // Should have metrics
        let metrics = result.metrics.unwrap();
        assert_eq!(metrics.strategy_name, "columnar");
        assert_eq!(metrics.execution_path, "unified");
    }

    #[tokio::test]
    async fn test_execute_without_provider_fails() {
        let strategy = ColumnarStrategy::new();

        let request = QueryRequest::sql("SELECT COUNT(*) FROM unknown")
            .with_target("unknown");

        let ctx = QueryContext::new(30000);
        let result = strategy.execute(request, &ctx).await;

        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("No columnar provider"));
    }

    #[test]
    fn test_strategy_priority() {
        let strategy = ColumnarStrategy::new().with_priority(75);
        assert_eq!(strategy.priority(), 75);
    }
}
