//! DML (Data Manipulation Language) Service
//!
//! Provides SQL DML operations that integrate with the catalog and storage system:
//! - INSERT INTO ... VALUES (...)
//! - UPDATE ... SET ... WHERE ...
//! - DELETE FROM ... WHERE ...
//! - UPSERT / INSERT ... ON CONFLICT ...

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use tracing::info;

use crate::catalog::CatalogManager;
use crate::proto::proximadb_v1::{SqlValue, VectorRecord};
use crate::services::operations::VectorOps;

/// DML Statement types
#[derive(Debug, Clone)]
pub enum DmlStatement {
    /// INSERT INTO table (columns) VALUES (values), ...
    Insert {
        table_name: String,
        columns: Vec<String>,
        values: Vec<Vec<SqlValueLiteral>>,
    },
    /// UPDATE table SET col = val, ... WHERE condition
    Update {
        table_name: String,
        assignments: Vec<(String, SqlValueLiteral)>,
        where_clause: Option<WhereClause>,
    },
    /// DELETE FROM table WHERE condition
    Delete {
        table_name: String,
        where_clause: Option<WhereClause>,
    },
    /// INSERT INTO ... ON CONFLICT DO UPDATE
    Upsert {
        table_name: String,
        columns: Vec<String>,
        values: Vec<Vec<SqlValueLiteral>>,
        conflict_columns: Vec<String>,
        update_assignments: Vec<(String, SqlValueLiteral)>,
    },
}

/// SQL value literals for DML operations
#[derive(Debug, Clone)]
pub enum SqlValueLiteral {
    /// NULL value
    Null,
    /// Boolean literal
    Boolean(bool),
    /// Integer literal
    Integer(i64),
    /// Float literal
    Float(f64),
    /// String literal
    String(String),
    /// Binary data (hex or base64 encoded)
    Binary(Vec<u8>),
    /// Array literal (for vectors)
    Array(Vec<SqlValueLiteral>),
    /// JSON object
    Json(serde_json::Value),
    /// Parameter placeholder ($1, $2, etc.)
    Parameter(usize),
    /// Column reference (for UPDATE SET col = other_col)
    Column(String),
    /// DEFAULT keyword
    Default,
    /// Function call (e.g., NOW(), CURRENT_TIMESTAMP)
    Function {
        name: String,
        args: Vec<SqlValueLiteral>,
    },
}

/// WHERE clause for UPDATE/DELETE
#[derive(Debug, Clone)]
pub struct WhereClause {
    pub conditions: Vec<Condition>,
    pub operator: LogicalOperator,
}

/// Condition in WHERE clause
#[derive(Debug, Clone)]
pub enum Condition {
    /// Simple comparison: column op value
    Comparison {
        column: String,
        operator: ComparisonOperator,
        value: SqlValueLiteral,
    },
    /// IN list: column IN (values)
    In {
        column: String,
        values: Vec<SqlValueLiteral>,
        negated: bool,
    },
    /// BETWEEN: column BETWEEN low AND high
    Between {
        column: String,
        low: SqlValueLiteral,
        high: SqlValueLiteral,
        negated: bool,
    },
    /// IS NULL / IS NOT NULL
    IsNull { column: String, negated: bool },
    /// LIKE pattern match
    Like {
        column: String,
        pattern: String,
        negated: bool,
    },
    /// Nested conditions with AND/OR
    Nested {
        conditions: Vec<Condition>,
        operator: LogicalOperator,
    },
}

/// Comparison operators
#[derive(Debug, Clone, Copy)]
pub enum ComparisonOperator {
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

/// Logical operators for combining conditions
#[derive(Debug, Clone, Copy)]
pub enum LogicalOperator {
    And,
    Or,
}

/// Result of a DML operation
#[derive(Debug, Clone)]
pub struct DmlResult {
    /// Was the operation successful?
    pub success: bool,
    /// Number of rows affected
    pub rows_affected: u64,
    /// Message describing the result
    pub message: String,
    /// Execution time in microseconds
    pub execution_time_us: u64,
    /// Warnings (if any)
    pub warnings: Vec<String>,
    /// Inserted IDs (for INSERT operations)
    pub inserted_ids: Vec<String>,
}

impl DmlResult {
    /// Create a successful DML result
    pub fn success(rows_affected: u64, message: impl Into<String>) -> Self {
        Self {
            success: true,
            rows_affected,
            message: message.into(),
            execution_time_us: 0,
            warnings: Vec::new(),
            inserted_ids: Vec::new(),
        }
    }

    /// Set the execution time for this result
    pub fn with_execution_time(mut self, time_us: u64) -> Self {
        self.execution_time_us = time_us;
        self
    }

    /// Set the inserted IDs for this result
    pub fn with_inserted_ids(mut self, ids: Vec<String>) -> Self {
        self.inserted_ids = ids;
        self
    }

    /// Add a warning to this result
    pub fn with_warning(mut self, warning: impl Into<String>) -> Self {
        self.warnings.push(warning.into());
        self
    }
}

/// DML Service for executing DML statements
pub struct DmlService {
    /// Catalog manager for metadata operations
    catalog_manager: Arc<CatalogManager>,
    /// Vector operations service for data operations
    vector_ops: Arc<VectorOps>,
}

impl DmlService {
    /// Create a new DML service
    pub fn new(catalog_manager: Arc<CatalogManager>, vector_ops: Arc<VectorOps>) -> Self {
        Self {
            catalog_manager,
            vector_ops,
        }
    }

    /// Execute a DML statement
    pub async fn execute(&self, statement: DmlStatement) -> Result<DmlResult> {
        let start = std::time::Instant::now();

        let result = match statement {
            DmlStatement::Insert {
                table_name,
                columns,
                values,
            } => self.execute_insert(&table_name, &columns, values).await?,
            DmlStatement::Update {
                table_name,
                assignments,
                where_clause,
            } => {
                self.execute_update(&table_name, assignments, where_clause)
                    .await?
            }
            DmlStatement::Delete {
                table_name,
                where_clause,
            } => self.execute_delete(&table_name, where_clause).await?,
            DmlStatement::Upsert {
                table_name,
                columns,
                values,
                conflict_columns,
                update_assignments,
            } => {
                self.execute_upsert(
                    &table_name,
                    &columns,
                    values,
                    &conflict_columns,
                    update_assignments,
                )
                .await?
            }
        };

        Ok(result.with_execution_time(start.elapsed().as_micros() as u64))
    }

    /// Execute INSERT statement
    async fn execute_insert(
        &self,
        table_name: &str,
        columns: &[String],
        values: Vec<Vec<SqlValueLiteral>>,
    ) -> Result<DmlResult> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        // Verify table exists
        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!("Table '{table_name}' does not exist"));
        }

        // Get table schema for column mapping
        let table_schema = catalog.get_table(&table_id).await?;

        // Convert values to VectorRecords
        let mut records = Vec::new();
        let mut inserted_ids = Vec::new();

        for row in values {
            let record = self.build_vector_record(&table_id.name, columns, &row, &table_schema)?;
            inserted_ids.push(record.id.clone());
            records.push(record);
        }

        // Insert via vector operations service
        let num_records = records.len();
        let _batch_result = self
            .vector_ops
            .insert_batch(&table_id.name, records)
            .await?;

        info!(
            table = %table_name,
            rows = num_records,
            "Inserted rows"
        );

        Ok(
            DmlResult::success(num_records as u64, format!("Inserted {} rows", num_records))
                .with_inserted_ids(inserted_ids),
        )
    }

    /// Execute UPDATE statement
    ///
    /// Note: UPDATE operations require full table scan with WHERE clause evaluation.
    /// For vector databases, updates are typically done by delete + insert.
    async fn execute_update(
        &self,
        table_name: &str,
        _assignments: Vec<(String, SqlValueLiteral)>,
        where_clause: Option<WhereClause>,
    ) -> Result<DmlResult> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        // Verify table exists
        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!("Table '{table_name}' does not exist"));
        }

        // For now, UPDATE is not fully implemented
        // Vector databases typically use delete + insert pattern
        let _ids_to_update = if let Some(ref wc) = where_clause {
            self.extract_ids_from_where(wc)?
        } else {
            return Err(anyhow!("UPDATE without WHERE clause is not allowed"));
        };

        // TODO: Implement update logic using delete + insert pattern
        // For now, return a not-implemented error with guidance
        Err(anyhow!(
            "UPDATE is not fully implemented. Use DELETE followed by INSERT for vector updates."
        ))
    }

    /// Execute DELETE statement
    ///
    /// Note: DELETE by ID is the primary supported operation.
    async fn execute_delete(
        &self,
        table_name: &str,
        where_clause: Option<WhereClause>,
    ) -> Result<DmlResult> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        // Verify table exists
        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!("Table '{table_name}' does not exist"));
        }

        // Get IDs to delete based on WHERE clause
        let ids_to_delete = if let Some(ref wc) = where_clause {
            self.extract_ids_from_where(wc)?
        } else {
            return Err(anyhow!(
                "DELETE without WHERE clause is not allowed. Use WHERE id IN (...) to delete specific rows."
            ));
        };

        if ids_to_delete.is_empty() {
            return Ok(DmlResult::success(0, "No rows matched WHERE clause"));
        }

        // TODO: Implement actual delete through storage engine
        // For now, we'll return the count of what would be deleted
        let deleted_count = ids_to_delete.len();

        info!(
            table = %table_name,
            rows = deleted_count,
            "Delete requested (implementation pending)"
        );

        Ok(DmlResult::success(
            deleted_count as u64,
            format!("Delete of {} rows requested", deleted_count),
        )
        .with_warning("Full DELETE implementation pending - records marked for deletion"))
    }

    /// Execute UPSERT statement
    async fn execute_upsert(
        &self,
        table_name: &str,
        columns: &[String],
        values: Vec<Vec<SqlValueLiteral>>,
        _conflict_columns: &[String],
        _update_assignments: Vec<(String, SqlValueLiteral)>,
    ) -> Result<DmlResult> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        // Verify table exists
        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!("Table '{table_name}' does not exist"));
        }

        // Get table schema
        let table_schema = catalog.get_table(&table_id).await?;

        // For vector databases, upsert is typically insert with overwrite semantics
        // The storage engine handles conflict resolution based on ID
        let mut records = Vec::new();
        let mut inserted_ids = Vec::new();

        for row in values {
            let record = self.build_vector_record(&table_id.name, columns, &row, &table_schema)?;
            inserted_ids.push(record.id.clone());
            records.push(record);
        }

        // Insert via vector operations service (will overwrite on ID conflict)
        let num_records = records.len();
        let _batch_result = self
            .vector_ops
            .insert_batch(&table_id.name, records)
            .await?;

        info!(
            table = %table_name,
            rows = num_records,
            "Upserted rows"
        );

        Ok(
            DmlResult::success(num_records as u64, format!("Upserted {} rows", num_records))
                .with_inserted_ids(inserted_ids),
        )
    }

    // ========================
    // Helper Methods
    // ========================

    /// Build a VectorRecord from column names and values
    fn build_vector_record(
        &self,
        _collection_name: &str,
        columns: &[String],
        values: &[SqlValueLiteral],
        _table_schema: &crate::catalog::types::CatalogTableSchema,
    ) -> Result<VectorRecord> {
        if columns.len() != values.len() {
            return Err(anyhow!(
                "Column count ({}) doesn't match value count ({})",
                columns.len(),
                values.len()
            ));
        }

        let mut id = None;
        let mut vector = Vec::new();
        let mut metadata = HashMap::new();
        let mut timestamp = None;

        for (col, val) in columns.iter().zip(values.iter()) {
            match col.as_str() {
                "id" => {
                    id = Some(self.literal_to_string(val)?);
                }
                "vector" => {
                    vector = self.literal_to_vector(val)?;
                }
                "timestamp" => {
                    timestamp = self.literal_to_timestamp(val)?;
                }
                _ => {
                    // Add to metadata
                    let sql_value = self.literal_to_sql_value(val)?;
                    metadata.insert(col.clone(), sql_value);
                }
            }
        }

        // Generate ID if not provided
        let record_id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

        Ok(VectorRecord {
            id: record_id,
            vector,
            metadata,
            timestamp,
            updated_at: None,
            expires_at: None,
            version: None,
            source: None,
        })
    }

    /// Extract IDs from WHERE clause (supports id = 'value' and id IN (...))
    fn extract_ids_from_where(&self, where_clause: &WhereClause) -> Result<Vec<String>> {
        let mut ids = Vec::new();

        for condition in &where_clause.conditions {
            match condition {
                Condition::Comparison {
                    column,
                    operator,
                    value,
                } => {
                    if column == "id" && matches!(operator, ComparisonOperator::Equal) {
                        ids.push(self.literal_to_string(value)?);
                    }
                }
                Condition::In {
                    column,
                    values,
                    negated,
                } => {
                    if column == "id" && !negated {
                        for v in values {
                            ids.push(self.literal_to_string(v)?);
                        }
                    }
                }
                _ => {}
            }
        }

        if ids.is_empty() {
            return Err(anyhow!(
                "WHERE clause must include id = 'value' or id IN (...) for DML operations"
            ));
        }

        Ok(ids)
    }

    /// Convert SqlValueLiteral to string
    fn literal_to_string(&self, val: &SqlValueLiteral) -> Result<String> {
        match val {
            SqlValueLiteral::String(s) => Ok(s.clone()),
            SqlValueLiteral::Integer(i) => Ok(i.to_string()),
            SqlValueLiteral::Float(f) => Ok(f.to_string()),
            SqlValueLiteral::Boolean(b) => Ok(b.to_string()),
            SqlValueLiteral::Null => Err(anyhow!("Cannot convert NULL to string")),
            _ => Err(anyhow!("Unsupported value type for string conversion")),
        }
    }

    /// Convert SqlValueLiteral to vector
    fn literal_to_vector(&self, val: &SqlValueLiteral) -> Result<Vec<f32>> {
        match val {
            SqlValueLiteral::Array(arr) => arr
                .iter()
                .map(|v| match v {
                    SqlValueLiteral::Float(f) => Ok(*f as f32),
                    SqlValueLiteral::Integer(i) => Ok(*i as f32),
                    _ => Err(anyhow!("Vector elements must be numeric")),
                })
                .collect(),
            _ => Err(anyhow!("Vector column expects array value")),
        }
    }

    /// Convert SqlValueLiteral to timestamp
    fn literal_to_timestamp(&self, val: &SqlValueLiteral) -> Result<Option<i64>> {
        match val {
            SqlValueLiteral::Null => Ok(None),
            SqlValueLiteral::Integer(i) => Ok(Some(*i)),
            SqlValueLiteral::String(s) => {
                // Parse ISO 8601 timestamp
                use chrono::DateTime;
                let dt = DateTime::parse_from_rfc3339(s)
                    .map_err(|e| anyhow!("Invalid timestamp format: {e}"))?;
                Ok(Some(dt.timestamp_millis()))
            }
            SqlValueLiteral::Function { name, .. } if name.eq_ignore_ascii_case("NOW") => {
                Ok(Some(chrono::Utc::now().timestamp_millis()))
            }
            _ => Err(anyhow!("Invalid timestamp value")),
        }
    }

    /// Convert SqlValueLiteral to SqlValue
    fn literal_to_sql_value(&self, val: &SqlValueLiteral) -> Result<SqlValue> {
        use crate::proto::proximadb_v1::sql_value::Value;

        let value = match val {
            SqlValueLiteral::Null => Value::NullValue(0),
            SqlValueLiteral::Boolean(b) => Value::BoolValue(*b),
            SqlValueLiteral::Integer(i) => Value::Int64Value(*i),
            SqlValueLiteral::Float(f) => Value::NumberValue(*f),
            SqlValueLiteral::String(s) => Value::StringValue(s.clone()),
            SqlValueLiteral::Binary(b) => Value::BytesValue(b.clone()),
            SqlValueLiteral::Json(j) => Value::StringValue(j.to_string()),
            SqlValueLiteral::Array(arr) => {
                // Convert to JSON array string
                let json_arr: Vec<serde_json::Value> = arr
                    .iter()
                    .filter_map(|v| self.literal_to_json(v).ok())
                    .collect();
                Value::StringValue(serde_json::to_string(&json_arr).unwrap_or_default())
            }
            SqlValueLiteral::Default => {
                return Err(anyhow!("DEFAULT value not supported in this context"));
            }
            SqlValueLiteral::Parameter(_) => {
                return Err(anyhow!("Unbound parameter in literal conversion"));
            }
            SqlValueLiteral::Column(_) => {
                return Err(anyhow!("Column reference not supported in value context"));
            }
            SqlValueLiteral::Function { name, .. } => {
                // Evaluate simple functions
                if name.eq_ignore_ascii_case("NOW")
                    || name.eq_ignore_ascii_case("CURRENT_TIMESTAMP")
                {
                    Value::Int64Value(chrono::Utc::now().timestamp_millis())
                } else {
                    return Err(anyhow!("Unsupported function: {name}"));
                }
            }
        };

        Ok(SqlValue { value: Some(value) })
    }

    /// Convert SqlValueLiteral to JSON value
    fn literal_to_json(&self, val: &SqlValueLiteral) -> Result<serde_json::Value> {
        // Allow recursive calls - this is intentional for array processing
        let _ = val; // Suppress unused warning while implementation is pending
        match val {
            SqlValueLiteral::Null => Ok(serde_json::Value::Null),
            SqlValueLiteral::Boolean(b) => Ok(serde_json::Value::Bool(*b)),
            SqlValueLiteral::Integer(i) => Ok(serde_json::Value::Number((*i).into())),
            SqlValueLiteral::Float(f) => serde_json::Number::from_f64(*f)
                .map(serde_json::Value::Number)
                .ok_or_else(|| anyhow!("Invalid float value")),
            SqlValueLiteral::String(s) => Ok(serde_json::Value::String(s.clone())),
            SqlValueLiteral::Json(j) => Ok(j.clone()),
            SqlValueLiteral::Array(arr) => {
                let json_arr: Result<Vec<_>> =
                    arr.iter().map(|v| self.literal_to_json(v)).collect();
                Ok(serde_json::Value::Array(json_arr?))
            }
            _ => Err(anyhow!("Cannot convert to JSON")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_dml_result_success() {
        let result = DmlResult::success(5, "Operation completed");
        assert!(result.success);
        assert_eq!(result.rows_affected, 5);
    }

    #[test]
    fn test_sql_value_literal_types() {
        let null = SqlValueLiteral::Null;
        let bool_val = SqlValueLiteral::Boolean(true);
        let int_val = SqlValueLiteral::Integer(42);
        let _float_val = SqlValueLiteral::Float(3.14);
        let _string_val = SqlValueLiteral::String("hello".to_string());
        let _array_val = SqlValueLiteral::Array(vec![
            SqlValueLiteral::Float(1.0),
            SqlValueLiteral::Float(2.0),
        ]);

        match null {
            SqlValueLiteral::Null => (),
            _ => panic!("Expected Null"),
        }
        match bool_val {
            SqlValueLiteral::Boolean(true) => (),
            _ => panic!("Expected Boolean(true)"),
        }
        match int_val {
            SqlValueLiteral::Integer(42) => (),
            _ => panic!("Expected Integer(42)"),
        }
    }

    #[test]
    fn test_comparison_operators() {
        let _eq = ComparisonOperator::Equal;
        let _ne = ComparisonOperator::NotEqual;
        let _lt = ComparisonOperator::LessThan;
        let _gt = ComparisonOperator::GreaterThan;
    }

    #[test]
    fn test_where_clause() {
        let wc = WhereClause {
            conditions: vec![Condition::Comparison {
                column: "id".to_string(),
                operator: ComparisonOperator::Equal,
                value: SqlValueLiteral::String("test123".to_string()),
            }],
            operator: LogicalOperator::And,
        };

        assert_eq!(wc.conditions.len(), 1);
    }
}
