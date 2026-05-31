//! # SEQUOIA Relational Storage Engine
//!
//! **STATUS**: Phase 1 (Apr 2026)
//!
//! SEQUOIA is a relational row-store engine that stores typed rows validated
//! against a catalog schema. It implements `RelationalStorageEngine` (the
//! relational-native trait) for efficient SQL-like table CRUD with filtering,
//! projection, ordering, and limit/offset.
//!
//! It also implements `UnifiedStorageEngine` as a thin stub for factory
//! registration, but all real relational operations go through
//! `RelationalStorageEngine`.

use anyhow::Result;
use async_trait::async_trait;
use dashmap::DashMap;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::core::search::results::OptimizedSearchRecord;
use crate::storage::persistence::filesystem::{FilesystemConfig, FilesystemFactory};
use crate::storage::traits::{
    CompactionParameters, CompactionResult, FlushParameters, FlushResult, StorageFormatStrategy,
    StorageQueryContext, UnifiedStorageEngine,
};

// ---------------------------------------------------------------------------
// Value types
// ---------------------------------------------------------------------------

/// A single typed value in a relational row.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum TypedValue {
    Null,
    Boolean(bool),
    Int32(i32),
    Int64(i64),
    Float32(f32),
    Float64(f64),
    String(String),
    Binary(Vec<u8>),
    Timestamp(i64),
    Json(serde_json::Value),
    Vector(Vec<f32>),
}

impl TypedValue {
    /// Attempt a partial ordering between two typed values for comparison filters.
    ///
    /// Returns `None` when the types are incompatible or the variant does not
    /// support ordering (e.g. Json, Binary, Vector).
    fn partial_cmp_typed(&self, other: &TypedValue) -> Option<std::cmp::Ordering> {
        match (self, other) {
            (TypedValue::Null, TypedValue::Null) => Some(std::cmp::Ordering::Equal),
            (TypedValue::Boolean(a), TypedValue::Boolean(b)) => a.partial_cmp(b),
            (TypedValue::Int32(a), TypedValue::Int32(b)) => a.partial_cmp(b),
            (TypedValue::Int64(a), TypedValue::Int64(b)) => a.partial_cmp(b),
            (TypedValue::Float32(a), TypedValue::Float32(b)) => a.partial_cmp(b),
            (TypedValue::Float64(a), TypedValue::Float64(b)) => a.partial_cmp(b),
            (TypedValue::String(a), TypedValue::String(b)) => a.partial_cmp(b),
            (TypedValue::Timestamp(a), TypedValue::Timestamp(b)) => a.partial_cmp(b),
            // Cross-numeric promotions (int32 <-> int64)
            (TypedValue::Int32(a), TypedValue::Int64(b)) => (*a as i64).partial_cmp(b),
            (TypedValue::Int64(a), TypedValue::Int32(b)) => a.partial_cmp(&(*b as i64)),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// Row types
// ---------------------------------------------------------------------------

/// A single row of typed values, column-positional.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct TypedRow {
    pub values: Vec<TypedValue>,
}

// ---------------------------------------------------------------------------
// Query types
// ---------------------------------------------------------------------------

/// Parameters for a relational query (SELECT-like).
#[derive(Debug, Clone, Default)]
pub struct RelationalQueryParams {
    /// Projection columns. Empty means all columns.
    pub columns: Vec<String>,
    /// Optional row filter (WHERE clause).
    pub filter: Option<RowFilter>,
    /// ORDER BY: (column_name, ascending).
    pub order_by: Vec<(String, bool)>,
    /// Maximum number of rows to return.
    pub limit: Option<u32>,
    /// Number of rows to skip before returning.
    pub offset: u32,
}

/// A recursive filter expression for row-level predicates.
#[derive(Debug, Clone)]
pub enum RowFilter {
    Eq(String, TypedValue),
    Ne(String, TypedValue),
    Gt(String, TypedValue),
    Lt(String, TypedValue),
    Gte(String, TypedValue),
    Lte(String, TypedValue),
    And(Vec<RowFilter>),
    Or(Vec<RowFilter>),
    IsNull(String),
    IsNotNull(String),
}

/// Result of a relational query.
#[derive(Debug, Clone)]
pub struct RelationalQueryResult {
    pub rows: Vec<TypedRow>,
    pub column_names: Vec<String>,
    pub total_count: Option<u64>,
    pub query_time_ms: u64,
}

// ---------------------------------------------------------------------------
// RelationalStorageEngine trait
// ---------------------------------------------------------------------------

/// Trait for relational (row-store) storage engines.
#[async_trait]
pub trait RelationalStorageEngine: Send + Sync {
    /// Engine identification.
    fn engine_name(&self) -> &'static str;

    // -- Schema DDL --

    /// Create a table with the given columns: (name, type_string).
    async fn create_table(&self, table: String, columns: Vec<(String, String)>) -> Result<()>;

    /// Drop a table. Returns `true` if the table existed.
    async fn drop_table(&self, table: String) -> Result<bool>;

    /// Get column definitions for a table, or `None` if it does not exist.
    async fn get_table_columns(&self, table: String) -> Result<Option<Vec<(String, String)>>>;

    // -- Row CRUD --

    /// Insert rows into a table. Returns the number of rows inserted.
    async fn insert_rows(
        &self,
        table: String,
        column_names: Vec<String>,
        rows: Vec<TypedRow>,
    ) -> Result<u64>;

    /// Query rows from a table with optional filter, projection, ordering, limit/offset.
    async fn query_rows(
        &self,
        table: String,
        params: RelationalQueryParams,
    ) -> Result<RelationalQueryResult>;

    /// Update rows matching an optional filter. Returns the number of rows updated.
    async fn update_rows(
        &self,
        table: String,
        updates: Vec<(String, TypedValue)>,
        filter: Option<RowFilter>,
    ) -> Result<u64>;

    /// Delete rows matching an optional filter. Returns the number of rows deleted.
    async fn delete_rows(&self, table: String, filter: Option<RowFilter>) -> Result<u64>;

    // -- Counts --

    /// Return the number of rows in a table.
    async fn row_count(&self, table: String) -> Result<u64>;

    // -- Persistence --

    /// Flush in-memory data for a table to durable storage. Returns bytes written.
    async fn flush(&self, table: String) -> Result<u64>;

    /// Compact storage for a table. Returns bytes reclaimed.
    async fn compact(&self, table: String) -> Result<u64>;

    // -- Metrics --

    /// Collect engine-level metrics.
    async fn collect_metrics(&self) -> Result<HashMap<String, serde_json::Value>>;
}

// ---------------------------------------------------------------------------
// SequoiaEngine
// ---------------------------------------------------------------------------

/// SEQUOIA storage engine -- relational row-store.
///
/// Uses DashMap for lock-free concurrent row access in the memtable.
/// Each table gets its own row vector.
pub struct SequoiaEngine {
    /// Table schemas: table_name -> Vec<(column_name, column_type)>
    schemas: DashMap<String, Vec<(String, String)>>,
    /// Table data: table_name -> Vec<TypedRow>
    tables: DashMap<String, Vec<TypedRow>>,
    /// Global row counter for metrics.
    row_count_metric: AtomicU64,
}

impl Default for SequoiaEngine {
    fn default() -> Self {
        Self {
            schemas: DashMap::new(),
            tables: DashMap::new(),
            row_count_metric: AtomicU64::new(0),
        }
    }
}

impl SequoiaEngine {
    /// Create a new `SequoiaEngine`.
    pub fn new() -> Self {
        Self::default()
    }
}

// ---------------------------------------------------------------------------
// Filter evaluation helper
// ---------------------------------------------------------------------------

/// Recursively evaluate a `RowFilter` against a single row.
fn evaluate_filter(row: &TypedRow, columns: &[(String, String)], filter: &RowFilter) -> bool {
    match filter {
        RowFilter::Eq(col, val) => {
            if let Some(idx) = columns.iter().position(|(name, _)| name == col) {
                row.values.get(idx) == Some(val)
            } else {
                false
            }
        }
        RowFilter::Ne(col, val) => {
            if let Some(idx) = columns.iter().position(|(name, _)| name == col) {
                row.values.get(idx).is_some_and(|v| v != val)
            } else {
                false
            }
        }
        RowFilter::Gt(col, val) => {
            if let Some(idx) = columns.iter().position(|(name, _)| name == col) {
                row.values.get(idx).and_then(|v| v.partial_cmp_typed(val))
                    == Some(std::cmp::Ordering::Greater)
            } else {
                false
            }
        }
        RowFilter::Lt(col, val) => {
            if let Some(idx) = columns.iter().position(|(name, _)| name == col) {
                row.values.get(idx).and_then(|v| v.partial_cmp_typed(val))
                    == Some(std::cmp::Ordering::Less)
            } else {
                false
            }
        }
        RowFilter::Gte(col, val) => {
            if let Some(idx) = columns.iter().position(|(name, _)| name == col) {
                row.values
                    .get(idx)
                    .and_then(|v| v.partial_cmp_typed(val))
                    .is_some_and(|ord| {
                        ord == std::cmp::Ordering::Greater || ord == std::cmp::Ordering::Equal
                    })
            } else {
                false
            }
        }
        RowFilter::Lte(col, val) => {
            if let Some(idx) = columns.iter().position(|(name, _)| name == col) {
                row.values
                    .get(idx)
                    .and_then(|v| v.partial_cmp_typed(val))
                    .is_some_and(|ord| {
                        ord == std::cmp::Ordering::Less || ord == std::cmp::Ordering::Equal
                    })
            } else {
                false
            }
        }
        RowFilter::And(filters) => filters.iter().all(|f| evaluate_filter(row, columns, f)),
        RowFilter::Or(filters) => filters.iter().any(|f| evaluate_filter(row, columns, f)),
        RowFilter::IsNull(col) => {
            if let Some(idx) = columns.iter().position(|(name, _)| name == col) {
                row.values
                    .get(idx)
                    .is_none_or(|v| matches!(v, TypedValue::Null))
            } else {
                false
            }
        }
        RowFilter::IsNotNull(col) => {
            if let Some(idx) = columns.iter().position(|(name, _)| name == col) {
                row.values
                    .get(idx)
                    .is_some_and(|v| !matches!(v, TypedValue::Null))
            } else {
                false
            }
        }
    }
}

// ---------------------------------------------------------------------------
// RelationalStorageEngine implementation
// ---------------------------------------------------------------------------

#[async_trait]
impl RelationalStorageEngine for SequoiaEngine {
    fn engine_name(&self) -> &'static str {
        "sequoia"
    }

    // -- Schema DDL --

    async fn create_table(&self, table: String, columns: Vec<(String, String)>) -> Result<()> {
        if self.schemas.contains_key(&table) {
            return Err(anyhow::anyhow!("Table '{}' already exists", table));
        }
        self.schemas.insert(table.clone(), columns);
        self.tables.insert(table, Vec::new());
        Ok(())
    }

    async fn drop_table(&self, table: String) -> Result<bool> {
        let removed_schema = self.schemas.remove(&table);
        if let Some((_, rows)) = self.tables.remove(&table) {
            self.row_count_metric
                .fetch_sub(rows.len() as u64, Ordering::Relaxed);
        }
        Ok(removed_schema.is_some())
    }

    async fn get_table_columns(&self, table: String) -> Result<Option<Vec<(String, String)>>> {
        Ok(self.schemas.get(&table).map(|r| r.value().clone()))
    }

    // -- Row CRUD --

    async fn insert_rows(
        &self,
        table: String,
        column_names: Vec<String>,
        rows: Vec<TypedRow>,
    ) -> Result<u64> {
        let schema = self
            .schemas
            .get(&table)
            .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table))?;

        let schema_cols: Vec<&str> = schema.iter().map(|(n, _)| n.as_str()).collect();

        // Validate that supplied column names match the schema columns (order and count).
        if column_names.len() != schema_cols.len() {
            return Err(anyhow::anyhow!(
                "Column count mismatch: expected {}, got {}",
                schema_cols.len(),
                column_names.len()
            ));
        }

        // Validate each row has the correct number of values.
        for (i, row) in rows.iter().enumerate() {
            if row.values.len() != schema_cols.len() {
                return Err(anyhow::anyhow!(
                    "Row {} has {} values but table '{}' has {} columns",
                    i,
                    row.values.len(),
                    table,
                    schema_cols.len()
                ));
            }
        }

        let count = rows.len() as u64;
        let mut table_data = self
            .tables
            .get_mut(&table)
            .ok_or_else(|| anyhow::anyhow!("Table '{}' data store missing", table))?;

        table_data.extend(rows);
        self.row_count_metric.fetch_add(count, Ordering::Relaxed);
        Ok(count)
    }

    async fn query_rows(
        &self,
        table: String,
        params: RelationalQueryParams,
    ) -> Result<RelationalQueryResult> {
        let start = std::time::Instant::now();

        let schema = self
            .schemas
            .get(&table)
            .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table))?;
        let columns: Vec<(String, String)> = schema.value().clone();

        let table_data = self
            .tables
            .get(&table)
            .ok_or_else(|| anyhow::anyhow!("Table '{}' data store missing", table))?;

        // 1. Filter
        let mut matched: Vec<TypedRow> = if let Some(ref filter) = params.filter {
            table_data
                .iter()
                .filter(|row| evaluate_filter(row, &columns, filter))
                .cloned()
                .collect()
        } else {
            table_data.clone()
        };

        let total_count = matched.len() as u64;

        // 2. Order By
        if !params.order_by.is_empty() {
            matched.sort_by(|a, b| {
                for (col_name, ascending) in &params.order_by {
                    if let Some(idx) = columns.iter().position(|(n, _)| n == col_name) {
                        let va = a.values.get(idx);
                        let vb = b.values.get(idx);
                        if let (Some(va), Some(vb)) = (va, vb)
                            && let Some(ord) = va.partial_cmp_typed(vb)
                        {
                            let ord = if *ascending { ord } else { ord.reverse() };
                            if ord != std::cmp::Ordering::Equal {
                                return ord;
                            }
                        }
                    }
                }
                std::cmp::Ordering::Equal
            });
        }

        // 3. Offset and Limit
        let offset = params.offset as usize;
        let rows_after_offset: Vec<TypedRow> = matched.into_iter().skip(offset).collect();
        let rows_limited: Vec<TypedRow> = if let Some(limit) = params.limit {
            rows_after_offset.into_iter().take(limit as usize).collect()
        } else {
            rows_after_offset
        };

        // 4. Projection
        let (result_rows, result_columns) = if params.columns.is_empty() {
            // All columns
            let col_names: Vec<String> = columns.iter().map(|(n, _)| n.clone()).collect();
            (rows_limited, col_names)
        } else {
            // Resolve projected column indices
            let proj_indices: Vec<usize> = params
                .columns
                .iter()
                .filter_map(|c| columns.iter().position(|(n, _)| n == c))
                .collect();

            let projected: Vec<TypedRow> = rows_limited
                .into_iter()
                .map(|row| {
                    let values = proj_indices
                        .iter()
                        .map(|&idx| row.values.get(idx).cloned().unwrap_or(TypedValue::Null))
                        .collect();
                    TypedRow { values }
                })
                .collect();

            (projected, params.columns.clone())
        };

        Ok(RelationalQueryResult {
            rows: result_rows,
            column_names: result_columns,
            total_count: Some(total_count),
            query_time_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn update_rows(
        &self,
        table: String,
        updates: Vec<(String, TypedValue)>,
        filter: Option<RowFilter>,
    ) -> Result<u64> {
        let schema = self
            .schemas
            .get(&table)
            .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table))?;
        let columns: Vec<(String, String)> = schema.value().clone();

        let mut table_data = self
            .tables
            .get_mut(&table)
            .ok_or_else(|| anyhow::anyhow!("Table '{}' data store missing", table))?;

        // Pre-compute update column indices
        let update_indices: Vec<(usize, TypedValue)> = updates
            .iter()
            .filter_map(|(col, val)| {
                columns
                    .iter()
                    .position(|(n, _)| n == col)
                    .map(|idx| (idx, val.clone()))
            })
            .collect();

        let mut updated_count: u64 = 0;

        for row in table_data.iter_mut() {
            let matches = match &filter {
                Some(f) => evaluate_filter(row, &columns, f),
                None => true,
            };
            if matches {
                for (idx, val) in &update_indices {
                    if *idx < row.values.len() {
                        row.values[*idx] = val.clone();
                    }
                }
                updated_count += 1;
            }
        }

        Ok(updated_count)
    }

    async fn delete_rows(&self, table: String, filter: Option<RowFilter>) -> Result<u64> {
        let schema = self
            .schemas
            .get(&table)
            .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table))?;
        let columns: Vec<(String, String)> = schema.value().clone();

        let mut table_data = self
            .tables
            .get_mut(&table)
            .ok_or_else(|| anyhow::anyhow!("Table '{}' data store missing", table))?;

        let before_len = table_data.len() as u64;

        match &filter {
            Some(f) => {
                table_data.retain(|row| !evaluate_filter(row, &columns, f));
            }
            None => {
                table_data.clear();
            }
        }

        let after_len = table_data.len() as u64;
        let deleted = before_len - after_len;
        self.row_count_metric.fetch_sub(deleted, Ordering::Relaxed);
        Ok(deleted)
    }

    async fn row_count(&self, table: String) -> Result<u64> {
        let table_data = self
            .tables
            .get(&table)
            .ok_or_else(|| anyhow::anyhow!("Table '{}' does not exist", table))?;
        Ok(table_data.len() as u64)
    }

    async fn flush(&self, _table: String) -> Result<u64> {
        // Phase 2: implement disk persistence
        Ok(0)
    }

    async fn compact(&self, _table: String) -> Result<u64> {
        // Phase 2: implement compaction
        Ok(0)
    }

    async fn collect_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        let mut metrics = HashMap::new();
        metrics.insert("engine".to_string(), serde_json::json!("sequoia"));
        metrics.insert(
            "row_count".to_string(),
            serde_json::json!(self.row_count_metric.load(Ordering::Relaxed)),
        );
        metrics.insert(
            "table_count".to_string(),
            serde_json::json!(self.schemas.len()),
        );
        Ok(metrics)
    }
}

// ---------------------------------------------------------------------------
// UnifiedStorageEngine stub (for factory registration only)
// ---------------------------------------------------------------------------

#[async_trait]
impl UnifiedStorageEngine for SequoiaEngine {
    fn engine_name(&self) -> &'static str {
        "sequoia"
    }

    fn engine_version(&self) -> &'static str {
        "0.1.0"
    }

    fn strategy(&self) -> StorageFormatStrategy {
        // No Sequoia variant yet; use Sst as the closest match.
        StorageFormatStrategy::Sst
    }

    fn get_filesystem_factory(&self) -> &FilesystemFactory {
        use std::sync::OnceLock;
        static FACTORY: OnceLock<FilesystemFactory> = OnceLock::new();
        FACTORY.get_or_init(|| {
            futures::executor::block_on(async {
                FilesystemFactory::create(FilesystemConfig::default())
                    .await
                    .unwrap_or_else(|_| {
                        #[allow(clippy::panic)]
                        {
                            panic!("Failed to create filesystem factory for SEQUOIA engine")
                        }
                    })
            })
        })
    }

    async fn collect_engine_metrics(&self) -> Result<HashMap<String, serde_json::Value>> {
        RelationalStorageEngine::collect_metrics(self).await
    }

    async fn vector_by_id(
        &self,
        _collection_id: &str,
        _base_path: &str,
        _vector_id: &str,
    ) -> Result<Option<proximadb_records::ProximaRecord>> {
        Ok(None) // SEQUOIA stores relational rows, not vectors
    }

    async fn search_vectors_unified(
        &self,
        _ctx: &StorageQueryContext,
    ) -> Result<Vec<OptimizedSearchRecord>> {
        Ok(vec![]) // Use RelationalStorageEngine methods for row queries
    }

    async fn do_flush(&self, _params: &FlushParameters) -> Result<FlushResult> {
        Ok(FlushResult::default())
    }

    async fn do_compact(&self, _params: &CompactionParameters) -> Result<CompactionResult> {
        Ok(CompactionResult::default())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a SEQUOIA engine with a "users" table (id INT64, name STRING, age INT32).
    async fn make_users_engine() -> SequoiaEngine {
        let engine = SequoiaEngine::new();
        engine
            .create_table(
                "users".to_string(),
                vec![
                    ("id".to_string(), "INT64".to_string()),
                    ("name".to_string(), "STRING".to_string()),
                    ("age".to_string(), "INT32".to_string()),
                ],
            )
            .await
            .expect("create_table should succeed");
        engine
    }

    /// Helper: insert a user row.
    fn user_row(id: i64, name: &str, age: i32) -> TypedRow {
        TypedRow {
            values: vec![
                TypedValue::Int64(id),
                TypedValue::String(name.to_string()),
                TypedValue::Int32(age),
            ],
        }
    }

    /// Helper: column names for users table.
    fn user_columns() -> Vec<String> {
        vec!["id".to_string(), "name".to_string(), "age".to_string()]
    }

    // -- test_sequoia_create_table --

    #[tokio::test]
    async fn test_sequoia_create_table() {
        let engine = SequoiaEngine::new();
        engine
            .create_table(
                "users".to_string(),
                vec![
                    ("id".to_string(), "INT64".to_string()),
                    ("name".to_string(), "STRING".to_string()),
                    ("age".to_string(), "INT32".to_string()),
                ],
            )
            .await
            .expect("create_table should succeed");

        let cols = engine
            .get_table_columns("users".to_string())
            .await
            .expect("get_table_columns should succeed");
        assert!(cols.is_some());
        let cols = cols.expect("columns should exist");
        assert_eq!(cols.len(), 3);
        assert_eq!(cols[0], ("id".to_string(), "INT64".to_string()));
        assert_eq!(cols[1], ("name".to_string(), "STRING".to_string()));
        assert_eq!(cols[2], ("age".to_string(), "INT32".to_string()));
    }

    // -- test_sequoia_insert_and_query --

    #[tokio::test]
    async fn test_sequoia_insert_and_query() {
        let engine = make_users_engine().await;

        let rows = vec![
            user_row(1, "Alice", 30),
            user_row(2, "Bob", 25),
            user_row(3, "Carol", 35),
        ];
        let inserted = engine
            .insert_rows("users".to_string(), user_columns(), rows)
            .await
            .expect("insert_rows should succeed");
        assert_eq!(inserted, 3);

        let result = engine
            .query_rows("users".to_string(), RelationalQueryParams::default())
            .await
            .expect("query_rows should succeed");
        assert_eq!(result.rows.len(), 3);
        assert_eq!(result.total_count, Some(3));
    }

    // -- test_sequoia_query_with_filter --

    #[tokio::test]
    async fn test_sequoia_query_with_filter() {
        let engine = make_users_engine().await;

        let rows = vec![
            user_row(1, "Alice", 30),
            user_row(2, "Bob", 25),
            user_row(3, "Carol", 35),
            user_row(4, "Dave", 20),
        ];
        engine
            .insert_rows("users".to_string(), user_columns(), rows)
            .await
            .expect("insert_rows should succeed");

        // WHERE age > 25
        let result = engine
            .query_rows(
                "users".to_string(),
                RelationalQueryParams {
                    filter: Some(RowFilter::Gt("age".to_string(), TypedValue::Int32(25))),
                    ..Default::default()
                },
            )
            .await
            .expect("query_rows should succeed");

        // Alice (30) and Carol (35) match
        assert_eq!(result.rows.len(), 2);
        assert_eq!(result.total_count, Some(2));
    }

    // -- test_sequoia_update_rows --

    #[tokio::test]
    async fn test_sequoia_update_rows() {
        let engine = make_users_engine().await;

        let rows = vec![user_row(1, "Alice", 30), user_row(2, "Bob", 25)];
        engine
            .insert_rows("users".to_string(), user_columns(), rows)
            .await
            .expect("insert_rows should succeed");

        // UPDATE users SET name='Alice Updated' WHERE id=1
        let updated = engine
            .update_rows(
                "users".to_string(),
                vec![(
                    "name".to_string(),
                    TypedValue::String("Alice Updated".to_string()),
                )],
                Some(RowFilter::Eq("id".to_string(), TypedValue::Int64(1))),
            )
            .await
            .expect("update_rows should succeed");
        assert_eq!(updated, 1);

        // Verify the update
        let result = engine
            .query_rows(
                "users".to_string(),
                RelationalQueryParams {
                    filter: Some(RowFilter::Eq("id".to_string(), TypedValue::Int64(1))),
                    ..Default::default()
                },
            )
            .await
            .expect("query_rows should succeed");
        assert_eq!(result.rows.len(), 1);
        assert_eq!(
            result.rows[0].values[1],
            TypedValue::String("Alice Updated".to_string())
        );
    }

    // -- test_sequoia_delete_rows --

    #[tokio::test]
    async fn test_sequoia_delete_rows() {
        let engine = make_users_engine().await;

        let rows = vec![
            user_row(1, "Alice", 30),
            user_row(2, "Bob", 17),
            user_row(3, "Carol", 19),
            user_row(4, "Dave", 25),
        ];
        engine
            .insert_rows("users".to_string(), user_columns(), rows)
            .await
            .expect("insert_rows should succeed");

        // DELETE WHERE age < 20
        let deleted = engine
            .delete_rows(
                "users".to_string(),
                Some(RowFilter::Lt("age".to_string(), TypedValue::Int32(20))),
            )
            .await
            .expect("delete_rows should succeed");
        // Bob (17) and Carol (19) removed
        assert_eq!(deleted, 2);

        let count = engine
            .row_count("users".to_string())
            .await
            .expect("row_count should succeed");
        assert_eq!(count, 2);
    }

    // -- test_sequoia_row_count --

    #[tokio::test]
    async fn test_sequoia_row_count() {
        let engine = make_users_engine().await;

        let rows: Vec<TypedRow> = (1..=5)
            .map(|i| user_row(i, &format!("user_{}", i), 20 + i as i32))
            .collect();
        engine
            .insert_rows("users".to_string(), user_columns(), rows)
            .await
            .expect("insert_rows should succeed");

        let count = engine
            .row_count("users".to_string())
            .await
            .expect("row_count should succeed");
        assert_eq!(count, 5);
    }

    // -- test_sequoia_query_with_projection --

    #[tokio::test]
    async fn test_sequoia_query_with_projection() {
        let engine = make_users_engine().await;

        let rows = vec![user_row(1, "Alice", 30), user_row(2, "Bob", 25)];
        engine
            .insert_rows("users".to_string(), user_columns(), rows)
            .await
            .expect("insert_rows should succeed");

        // SELECT name, age FROM users
        let result = engine
            .query_rows(
                "users".to_string(),
                RelationalQueryParams {
                    columns: vec!["name".to_string(), "age".to_string()],
                    ..Default::default()
                },
            )
            .await
            .expect("query_rows should succeed");

        assert_eq!(result.column_names, vec!["name", "age"]);
        assert_eq!(result.rows.len(), 2);
        // Each projected row should have exactly 2 values (name, age)
        for row in &result.rows {
            assert_eq!(row.values.len(), 2);
        }
        // First row: Alice, 30
        assert_eq!(
            result.rows[0].values[0],
            TypedValue::String("Alice".to_string())
        );
        assert_eq!(result.rows[0].values[1], TypedValue::Int32(30));
    }

    // -- test_sequoia_query_with_limit_offset --

    #[tokio::test]
    async fn test_sequoia_query_with_limit_offset() {
        let engine = make_users_engine().await;

        let rows: Vec<TypedRow> = (1..=10)
            .map(|i| user_row(i, &format!("user_{}", i), 20 + i as i32))
            .collect();
        engine
            .insert_rows("users".to_string(), user_columns(), rows)
            .await
            .expect("insert_rows should succeed");

        // SELECT * FROM users LIMIT 3 OFFSET 2
        let result = engine
            .query_rows(
                "users".to_string(),
                RelationalQueryParams {
                    limit: Some(3),
                    offset: 2,
                    ..Default::default()
                },
            )
            .await
            .expect("query_rows should succeed");

        assert_eq!(result.rows.len(), 3);
        // total_count reflects all rows (before limit/offset)
        assert_eq!(result.total_count, Some(10));
        // The first returned row should be the 3rd inserted (id=3)
        assert_eq!(result.rows[0].values[0], TypedValue::Int64(3));
    }

    // -- test_sequoia_drop_table --

    #[tokio::test]
    async fn test_sequoia_drop_table() {
        let engine = make_users_engine().await;

        // Insert a row so we know the table has data
        engine
            .insert_rows(
                "users".to_string(),
                user_columns(),
                vec![user_row(1, "Alice", 30)],
            )
            .await
            .expect("insert_rows should succeed");

        // Drop the table
        let dropped = engine
            .drop_table("users".to_string())
            .await
            .expect("drop_table should succeed");
        assert!(dropped);

        // Verify table is gone
        let cols = engine
            .get_table_columns("users".to_string())
            .await
            .expect("get_table_columns should succeed");
        assert!(cols.is_none());

        // Dropping again returns false
        let dropped_again = engine
            .drop_table("users".to_string())
            .await
            .expect("drop_table should succeed");
        assert!(!dropped_again);
    }

    // -- test_sequoia_engine_identity (UnifiedStorageEngine stub) --

    #[test]
    fn test_sequoia_engine_identity() {
        let engine = SequoiaEngine::new();
        assert_eq!(UnifiedStorageEngine::engine_name(&engine), "sequoia");
        assert_eq!(engine.engine_version(), "0.1.0");
        assert_eq!(engine.strategy(), StorageFormatStrategy::Sst);
    }
}
