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
use proximadb_catalog::{
    CatalogDataType, CatalogTableSchema,
    relational::{CatalogRow, RelationalWriteProfile},
};
use proximadb_data_model::ProximaValue;
use proximadb_records::{EmbeddingCell, ProximaRecord, ProximaTreeNode};
use tracing::info;

use crate::catalog::CatalogManager;
use crate::services::operations::VectorOps;
use crate::services::operations::vectors::{RichRecordGetRequest, RichSearchResult};

/// DML Statement types
#[derive(Debug, Clone)]
pub enum DmlStatement {
    /// INSERT INTO table (columns) VALUES (values), ...
    Insert {
        /// Target table name.
        table_name: String,
        /// Column names in insertion order.
        columns: Vec<String>,
        /// Rows to insert; each inner `Vec` is one row of values.
        values: Vec<Vec<SqlValueLiteral>>,
    },
    /// UPDATE table SET col = val, ... WHERE condition
    Update {
        /// Target table name.
        table_name: String,
        /// Column-value pairs to set.
        assignments: Vec<(String, SqlValueLiteral)>,
        /// Optional filter restricting which rows are updated.
        where_clause: Option<WhereClause>,
    },
    /// DELETE FROM table WHERE condition
    Delete {
        /// Target table name.
        table_name: String,
        /// Optional filter restricting which rows are deleted.
        where_clause: Option<WhereClause>,
    },
    /// INSERT INTO ... ON CONFLICT DO UPDATE
    Upsert {
        /// Target table name.
        table_name: String,
        /// Column names in insertion order.
        columns: Vec<String>,
        /// Rows to insert or update; each inner `Vec` is one row of values.
        values: Vec<Vec<SqlValueLiteral>>,
        /// Columns that define the conflict (unique key).
        conflict_columns: Vec<String>,
        /// Column-value pairs applied when a conflict is detected.
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
        /// SQL function name (case-insensitive).
        name: String,
        /// Arguments passed to the function.
        args: Vec<SqlValueLiteral>,
    },
}

/// WHERE clause for UPDATE/DELETE
#[derive(Debug, Clone)]
pub struct WhereClause {
    /// Individual conditions that make up the clause.
    pub conditions: Vec<Condition>,
    /// How conditions are combined (AND / OR).
    pub operator: LogicalOperator,
}

/// Condition in WHERE clause
#[derive(Debug, Clone)]
pub enum Condition {
    /// Simple comparison: column op value
    Comparison {
        /// Column name to compare.
        column: String,
        /// Comparison operator to apply.
        operator: ComparisonOperator,
        /// Right-hand side value.
        value: SqlValueLiteral,
    },
    /// IN list: column IN (values)
    In {
        /// Column name to test.
        column: String,
        /// Set of values to test membership against.
        values: Vec<SqlValueLiteral>,
        /// When `true`, the condition is `NOT IN`.
        negated: bool,
    },
    /// BETWEEN: column BETWEEN low AND high
    Between {
        /// Column name to test.
        column: String,
        /// Lower bound (inclusive).
        low: SqlValueLiteral,
        /// Upper bound (inclusive).
        high: SqlValueLiteral,
        /// When `true`, the condition is `NOT BETWEEN`.
        negated: bool,
    },
    /// IS NULL / IS NOT NULL
    IsNull {
        /// Column name to test.
        column: String,
        /// When `true`, the condition is `IS NOT NULL`.
        negated: bool,
    },
    /// LIKE pattern match
    Like {
        /// Column name to test.
        column: String,
        /// SQL LIKE pattern (supports `%` and `_` wildcards).
        pattern: String,
        /// When `true`, the condition is `NOT LIKE`.
        negated: bool,
    },
    /// Nested conditions with AND/OR
    Nested {
        /// Inner conditions to combine.
        conditions: Vec<Condition>,
        /// Logical operator applied to inner conditions.
        operator: LogicalOperator,
    },
}

/// Comparison operators
#[derive(Debug, Clone, Copy)]
pub enum ComparisonOperator {
    /// `=`
    Equal,
    /// `<>` or `!=`
    NotEqual,
    /// `<`
    LessThan,
    /// `<=`
    LessThanOrEqual,
    /// `>`
    GreaterThan,
    /// `>=`
    GreaterThanOrEqual,
}

/// Logical operators for combining conditions
#[derive(Debug, Clone, Copy)]
pub enum LogicalOperator {
    /// All conditions must be satisfied (SQL `AND`).
    And,
    /// At least one condition must be satisfied (SQL `OR`).
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

        // Convert SQL literals into canonical ProximaRecord envelopes.
        let mut records = Vec::new();
        let mut inserted_ids = Vec::new();

        for row in values {
            let record = self.build_proxima_record(columns, &row, &table_schema)?;
            inserted_ids.push(record.oid.clone());
            records.push(record);
        }

        // Insert canonical records. VectorRecord adaptation remains behind VectorOps until
        // storage/WAL accept ProximaRecord directly.
        let num_records = records.len();
        let batch_result = self
            .vector_ops
            .insert_records_with_tenant_context(&table_id.name, records, None)
            .await?;
        if !batch_result.success {
            return Err(anyhow!(
                "Insert failed: {}",
                batch_result
                    .errors
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "unknown error".to_string())
            ));
        }

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
        assignments: Vec<(String, SqlValueLiteral)>,
        where_clause: Option<WhereClause>,
    ) -> Result<DmlResult> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        // Verify table exists
        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!("Table '{table_name}' does not exist"));
        }

        let table_schema = catalog.get_table(&table_id).await?;
        let ids_to_update = if let Some(ref wc) = where_clause {
            self.extract_ids_from_where(wc, &table_schema)?
        } else {
            return Err(anyhow!("UPDATE without WHERE clause is not allowed"));
        };
        if ids_to_update.is_empty() {
            return Ok(DmlResult::success(0, "No rows matched WHERE clause"));
        }
        Self::validate_update_assignments(&assignments, &table_schema)?;

        let mut records = Vec::new();
        let mut warnings = Vec::new();
        for record_id in &ids_to_update {
            let Some(existing) = self
                .vector_ops
                .get_record_with_tenant_context(
                    RichRecordGetRequest {
                        collection_id: table_id.name.clone(),
                        record_id: record_id.clone(),
                        include_vector: true,
                        include_props: true,
                    },
                    None,
                )
                .await?
            else {
                warnings.push(format!(
                    "Record '{}' was not found in table '{}'",
                    record_id, table_schema.name
                ));
                continue;
            };

            records.push(self.build_updated_proxima_record(
                existing,
                &assignments,
                &table_schema,
            )?);
        }

        if records.is_empty() {
            return Ok(DmlResult::success(0, "No rows matched WHERE clause"));
        }

        let updated_count = records.len();
        let batch_result = self
            .vector_ops
            .insert_records_with_tenant_context(&table_id.name, records, None)
            .await?;
        if !batch_result.success {
            return Err(anyhow!(
                "Update failed: {}",
                batch_result
                    .errors
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        info!(
            table = %table_name,
            rows = updated_count,
            "Updated rows"
        );

        let mut result = DmlResult::success(
            updated_count as u64,
            format!("Updated {} rows", updated_count),
        );
        result.warnings = warnings;
        Ok(result)
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

        let table_schema = catalog.get_table(&table_id).await?;

        // Get IDs to delete based on WHERE clause
        let ids_to_delete = if let Some(ref wc) = where_clause {
            self.extract_ids_from_where(wc, &table_schema)?
        } else {
            return Err(anyhow!(
                "DELETE without WHERE clause is not allowed. Use WHERE primary key IN (...) to delete specific rows."
            ));
        };

        if ids_to_delete.is_empty() {
            return Ok(DmlResult::success(0, "No rows matched WHERE clause"));
        }

        let deleted_count = ids_to_delete.len();
        let batch_result = self
            .vector_ops
            .delete_records_with_tenant_context(&table_id.name, ids_to_delete, None)
            .await?;
        if !batch_result.success {
            return Err(anyhow!(
                "Delete failed: {}",
                batch_result
                    .errors
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "unknown error".to_string())
            ));
        }

        info!(
            table = %table_name,
            rows = deleted_count,
            "Deleted rows"
        );

        Ok(DmlResult::success(
            deleted_count as u64,
            format!("Deleted {} rows", deleted_count),
        ))
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

        let mut records = Vec::new();
        let mut inserted_ids = Vec::new();

        for row in values {
            let record = self.build_proxima_record(columns, &row, &table_schema)?;
            inserted_ids.push(record.oid.clone());
            records.push(record);
        }

        let num_records = records.len();
        let batch_result = self
            .vector_ops
            .insert_records_with_tenant_context(&table_id.name, records, None)
            .await?;
        if !batch_result.success {
            return Err(anyhow!(
                "Upsert failed: {}",
                batch_result
                    .errors
                    .first()
                    .cloned()
                    .unwrap_or_else(|| "unknown error".to_string())
            ));
        }

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

    /// Build a canonical ProximaRecord from catalog schema and SQL literals.
    fn build_proxima_record(
        &self,
        columns: &[String],
        values: &[SqlValueLiteral],
        table_schema: &CatalogTableSchema,
    ) -> Result<ProximaRecord> {
        if columns.len() != values.len() {
            return Err(anyhow!(
                "Column count ({}) doesn't match value count ({})",
                columns.len(),
                values.len()
            ));
        }

        self.validate_insert_columns(columns, values, table_schema)?;

        let vector_column = table_schema
            .columns
            .iter()
            .find(|column| matches!(column.data_type, CatalogDataType::Vector))
            .map(|column| (column.name.clone(), column.properties.clone()));

        let mut row_values = HashMap::new();
        let mut props = HashMap::new();
        let mut embeddings = Vec::new();
        let mut created_at_ns = None;

        for (col, val) in columns.iter().zip(values.iter()) {
            let effective_value = self.effective_insert_literal(col, val, table_schema)?;
            let proxima_value =
                self.literal_to_proxima_value_for_column(col, &effective_value, table_schema)?;

            row_values.insert(col.clone(), proxima_value.clone());
            props.insert(col.clone(), ProximaTreeNode::Value(proxima_value.clone()));

            if vector_column.as_ref().is_some_and(|(name, _)| name == col) {
                let vector = match proxima_value {
                    ProximaValue::DenseVector(vector) => vector,
                    ProximaValue::Null => Vec::new(),
                    _ => self.literal_to_vector(&effective_value)?,
                };
                if let Some((_, properties)) = &vector_column {
                    if let Some(expected) = properties
                        .get("dimension")
                        .and_then(|dimension| dimension.parse::<usize>().ok())
                    {
                        if vector.len() != expected {
                            return Err(anyhow!(
                                "Vector column '{}' expects dimension {}, got {}",
                                col,
                                expected,
                                vector.len()
                            ));
                        }
                    }
                }
                if !vector.is_empty() {
                    embeddings.push(EmbeddingCell {
                        model_id: "default".to_string(),
                        modality: "vector".to_string(),
                        dim: vector.len() as u32,
                        values: vector,
                    });
                }
                continue;
            }

            if col == "timestamp" {
                created_at_ns = self
                    .literal_to_timestamp(&effective_value)?
                    .map(|timestamp_ms| timestamp_ms.saturating_mul(1_000_000));
                continue;
            }
        }

        for column in &table_schema.columns {
            if columns.iter().any(|provided| provided == &column.name) {
                continue;
            }
            let Some(default_value) = &column.default_value else {
                continue;
            };
            let default_literal = Self::parse_default_literal(default_value)?;
            let proxima_value = self.literal_to_proxima_value_for_column(
                &column.name,
                &default_literal,
                table_schema,
            )?;
            row_values.insert(column.name.clone(), proxima_value.clone());
            props.insert(
                column.name.clone(),
                ProximaTreeNode::Value(proxima_value.clone()),
            );

            if vector_column
                .as_ref()
                .is_some_and(|(name, _)| name == &column.name)
            {
                let vector = match proxima_value {
                    ProximaValue::DenseVector(vector) => vector,
                    ProximaValue::Null => Vec::new(),
                    _ => self.literal_to_vector(&default_literal)?,
                };
                if !vector.is_empty() {
                    embeddings.push(EmbeddingCell {
                        model_id: "default".to_string(),
                        modality: "vector".to_string(),
                        dim: vector.len() as u32,
                        values: vector,
                    });
                }
                continue;
            }
        }

        let catalog_row =
            CatalogRow::validate(table_schema, row_values, &RelationalWriteProfile::oltp())?;
        let record_id = catalog_row
            .primary_key_string(table_schema)?
            .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
        let mut record = ProximaRecord {
            oid: record_id.clone(),
            local_id: Some(record_id),
            props,
            embeddings,
            method: Some("sql_dml".to_string()),
            variation_id: Some(table_schema.name.clone()),
            ..ProximaRecord::default()
        };
        if let Some(created_at_ns) = created_at_ns {
            record.created_at_ns = created_at_ns;
            record.updated_at_ns = created_at_ns;
        }
        Ok(record)
    }

    fn validate_insert_columns(
        &self,
        columns: &[String],
        values: &[SqlValueLiteral],
        table_schema: &CatalogTableSchema,
    ) -> Result<()> {
        for column in columns {
            if !table_schema
                .columns
                .iter()
                .any(|schema_column| schema_column.name == *column)
            {
                return Err(anyhow!(
                    "Column '{}' does not exist in table '{}'",
                    column,
                    table_schema.name
                ));
            }
        }

        for schema_column in &table_schema.columns {
            if schema_column.nullable || schema_column.default_value.is_some() {
                continue;
            }
            let Some(position) = columns
                .iter()
                .position(|column| column == &schema_column.name)
            else {
                return Err(anyhow!(
                    "Column '{}' is required for table '{}'",
                    schema_column.name,
                    table_schema.name
                ));
            };
            if values.get(position).is_some_and(Self::literal_is_null) {
                return Err(anyhow!(
                    "Column '{}' cannot be NULL for table '{}'",
                    schema_column.name,
                    table_schema.name
                ));
            }
        }

        Ok(())
    }

    fn validate_update_assignments(
        assignments: &[(String, SqlValueLiteral)],
        table_schema: &CatalogTableSchema,
    ) -> Result<()> {
        if assignments.is_empty() {
            return Err(anyhow!("UPDATE requires at least one assignment"));
        }

        let primary_key_column = Self::primary_key_column(table_schema);
        for (column_name, value) in assignments {
            let Some(column) = table_schema
                .columns
                .iter()
                .find(|column| column.name == *column_name)
            else {
                return Err(anyhow!(
                    "Column '{}' does not exist in table '{}'",
                    column_name,
                    table_schema.name
                ));
            };
            if primary_key_column.as_deref() == Some(column_name.as_str()) {
                return Err(anyhow!(
                    "UPDATE cannot modify primary key column '{}'",
                    column_name
                ));
            }
            if Self::literal_is_null(value) && !column.nullable {
                return Err(anyhow!(
                    "Column '{}' cannot be NULL for table '{}'",
                    column.name,
                    table_schema.name
                ));
            }
            if matches!(value, SqlValueLiteral::Default) && column.default_value.is_none() {
                return Err(anyhow!("Column '{}' has no DEFAULT value", column_name));
            }
        }

        Ok(())
    }

    fn build_updated_proxima_record(
        &self,
        existing: RichSearchResult,
        assignments: &[(String, SqlValueLiteral)],
        table_schema: &CatalogTableSchema,
    ) -> Result<ProximaRecord> {
        let vector_column = table_schema
            .columns
            .iter()
            .find(|column| matches!(column.data_type, CatalogDataType::Vector))
            .map(|column| (column.name.clone(), column.properties.clone()));

        let mut props: HashMap<String, ProximaTreeNode> = existing
            .props
            .into_iter()
            .map(|(key, value)| (key, ProximaTreeNode::Value(value)))
            .collect();
        let mut embeddings = if existing.vector.is_empty() {
            Vec::new()
        } else {
            vec![EmbeddingCell {
                model_id: "default".to_string(),
                modality: "vector".to_string(),
                dim: existing.vector.len() as u32,
                values: existing.vector,
            }]
        };
        let mut created_at_ns = existing
            .timestamp
            .map(|timestamp_ms| timestamp_ms.saturating_mul(1_000_000));
        let mut updated_at_ns = chrono::Utc::now()
            .timestamp_millis()
            .saturating_mul(1_000_000);

        for (column_name, value) in assignments {
            let effective_value =
                self.effective_insert_literal(column_name, value, table_schema)?;

            if vector_column
                .as_ref()
                .is_some_and(|(name, _)| name == column_name)
            {
                if Self::literal_is_null(&effective_value) {
                    embeddings.clear();
                    continue;
                }
                let vector = self.literal_to_vector(&effective_value)?;
                if let Some((_, properties)) = &vector_column
                    && let Some(expected) = properties
                        .get("dimension")
                        .and_then(|dimension| dimension.parse::<usize>().ok())
                    && vector.len() != expected
                {
                    return Err(anyhow!(
                        "Vector column '{}' expects dimension {}, got {}",
                        column_name,
                        expected,
                        vector.len()
                    ));
                }
                embeddings = vec![EmbeddingCell {
                    model_id: "default".to_string(),
                    modality: "vector".to_string(),
                    dim: vector.len() as u32,
                    values: vector,
                }];
                continue;
            }

            if column_name == "timestamp" {
                if let Some(timestamp_ms) = self.literal_to_timestamp(&effective_value)? {
                    let timestamp_ns = timestamp_ms.saturating_mul(1_000_000);
                    created_at_ns = Some(timestamp_ns);
                    updated_at_ns = timestamp_ns;
                }
                continue;
            }

            props.insert(
                column_name.clone(),
                ProximaTreeNode::Value(self.literal_to_proxima_value_for_column(
                    column_name,
                    &effective_value,
                    table_schema,
                )?),
            );
        }

        if let Some(primary_key_column) = Self::primary_key_column(table_schema) {
            props.entry(primary_key_column).or_insert_with(|| {
                ProximaTreeNode::Value(ProximaValue::String(existing.id.clone()))
            });
        }

        let mut record = ProximaRecord {
            oid: existing.id.clone(),
            local_id: Some(existing.id),
            props,
            embeddings,
            method: Some("sql_dml_update".to_string()),
            ..ProximaRecord::default()
        };
        if let Some(created_at_ns) = created_at_ns {
            record.created_at_ns = created_at_ns;
        }
        record.updated_at_ns = updated_at_ns;
        Ok(record)
    }

    fn literal_is_null(value: &SqlValueLiteral) -> bool {
        matches!(value, SqlValueLiteral::Null)
    }

    fn primary_key_column(table_schema: &CatalogTableSchema) -> Option<String> {
        table_schema.primary_key.first().cloned().or_else(|| {
            table_schema
                .columns
                .iter()
                .find(|column| column.name == "id" || column.name == "record_id")
                .map(|column| column.name.clone())
        })
    }

    /// Extract IDs from WHERE clause using the catalog primary key.
    fn extract_ids_from_where(
        &self,
        where_clause: &WhereClause,
        table_schema: &CatalogTableSchema,
    ) -> Result<Vec<String>> {
        let Some(primary_key_column) = Self::primary_key_column(table_schema) else {
            return Err(anyhow!(
                "Table '{}' has no single-column primary key/id column for DML key extraction",
                table_schema.name
            ));
        };
        let mut ids = Vec::new();

        for condition in &where_clause.conditions {
            match condition {
                Condition::Comparison {
                    column,
                    operator,
                    value,
                } => {
                    if column == &primary_key_column
                        && matches!(operator, ComparisonOperator::Equal)
                    {
                        ids.push(self.literal_to_string(value)?);
                    }
                }
                Condition::In {
                    column,
                    values,
                    negated,
                } => {
                    if column == &primary_key_column && !negated {
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
                "WHERE clause must include {} = 'value' or {} IN (...) for DML operations",
                primary_key_column,
                primary_key_column
            ));
        }

        Ok(ids)
    }

    fn effective_insert_literal(
        &self,
        column_name: &str,
        value: &SqlValueLiteral,
        table_schema: &CatalogTableSchema,
    ) -> Result<SqlValueLiteral> {
        if !matches!(value, SqlValueLiteral::Default) {
            return Ok(value.clone());
        }

        let Some(column) = table_schema
            .columns
            .iter()
            .find(|column| column.name == column_name)
        else {
            return Err(anyhow!("Column '{}' does not exist", column_name));
        };
        let Some(default_value) = &column.default_value else {
            return Err(anyhow!("Column '{}' has no DEFAULT value", column_name));
        };

        Self::parse_default_literal(default_value)
    }

    fn parse_default_literal(default_value: &str) -> Result<SqlValueLiteral> {
        let without_cast = default_value
            .split_once("::")
            .map(|(value, _)| value)
            .unwrap_or(default_value)
            .trim();
        let trimmed = without_cast
            .strip_prefix('(')
            .and_then(|value| value.strip_suffix(')'))
            .unwrap_or(without_cast)
            .trim();

        if trimmed.eq_ignore_ascii_case("NULL") {
            return Ok(SqlValueLiteral::Null);
        }
        if trimmed.eq_ignore_ascii_case("TRUE") {
            return Ok(SqlValueLiteral::Boolean(true));
        }
        if trimmed.eq_ignore_ascii_case("FALSE") {
            return Ok(SqlValueLiteral::Boolean(false));
        }
        if trimmed.eq_ignore_ascii_case("NOW()")
            || trimmed.eq_ignore_ascii_case("CURRENT_TIMESTAMP")
            || trimmed.eq_ignore_ascii_case("CURRENT_TIMESTAMP()")
        {
            return Ok(SqlValueLiteral::Function {
                name: "CURRENT_TIMESTAMP".to_string(),
                args: Vec::new(),
            });
        }

        if let Some(unquoted) = Self::unquote_sql_string(trimmed) {
            let value = unquoted?;
            if value.starts_with('{') || value.starts_with('[') {
                if let Ok(json) = serde_json::from_str(&value) {
                    return Ok(SqlValueLiteral::Json(json));
                }
            }
            return Ok(SqlValueLiteral::String(value));
        }

        if let Ok(value) = trimmed.parse::<i64>() {
            return Ok(SqlValueLiteral::Integer(value));
        }
        if let Ok(value) = trimmed.parse::<f64>() {
            return Ok(SqlValueLiteral::Float(value));
        }
        if trimmed.starts_with('{') || trimmed.starts_with('[') {
            if let Ok(json) = serde_json::from_str(trimmed) {
                return Ok(SqlValueLiteral::Json(json));
            }
        }

        Ok(SqlValueLiteral::String(trimmed.to_string()))
    }

    fn unquote_sql_string(value: &str) -> Option<Result<String>> {
        if !(value.len() >= 2 && value.starts_with('\'') && value.ends_with('\'')) {
            return None;
        }

        let mut output = String::new();
        let mut chars = value[1..value.len() - 1].chars().peekable();
        while let Some(ch) = chars.next() {
            if ch == '\'' {
                if chars.peek() == Some(&'\'') {
                    chars.next();
                    output.push('\'');
                } else {
                    return Some(Err(anyhow!("Invalid SQL string literal: {}", value)));
                }
            } else {
                output.push(ch);
            }
        }
        Some(Ok(output))
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
            SqlValueLiteral::String(value) => value
                .trim()
                .trim_start_matches('[')
                .trim_end_matches(']')
                .split(',')
                .filter(|part| !part.trim().is_empty())
                .map(|part| {
                    part.trim()
                        .parse::<f32>()
                        .map_err(|e| anyhow!("Invalid vector element '{}': {}", part, e))
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

    fn literal_to_proxima_value_for_column(
        &self,
        column_name: &str,
        val: &SqlValueLiteral,
        table_schema: &CatalogTableSchema,
    ) -> Result<ProximaValue> {
        let Some(column) = table_schema
            .columns
            .iter()
            .find(|column| column.name == column_name)
        else {
            return self.literal_to_proxima_value(val);
        };

        match column.data_type {
            CatalogDataType::Boolean => match val {
                SqlValueLiteral::Boolean(value) => Ok(ProximaValue::Boolean(*value)),
                SqlValueLiteral::Null if column.nullable => Ok(ProximaValue::Null),
                _ => Err(anyhow!("Column '{}' expects boolean", column_name)),
            },
            CatalogDataType::Int8 => self
                .literal_to_i64(val)
                .map(|v| ProximaValue::Int8(v as i8)),
            CatalogDataType::Int16 => self
                .literal_to_i64(val)
                .map(|v| ProximaValue::Int16(v as i16)),
            CatalogDataType::Int32 => self
                .literal_to_i64(val)
                .map(|v| ProximaValue::Int32(v as i32)),
            CatalogDataType::Int64 => self.literal_to_i64(val).map(ProximaValue::Int64),
            CatalogDataType::Float32 => self
                .literal_to_f64(val)
                .map(|v| ProximaValue::Float32(v as f32)),
            CatalogDataType::Float64 => self.literal_to_f64(val).map(ProximaValue::Float64),
            CatalogDataType::String | CatalogDataType::Uuid => {
                self.literal_to_string(val).map(ProximaValue::String)
            }
            CatalogDataType::Json => {
                let json = match val {
                    SqlValueLiteral::Json(value) => value.clone(),
                    SqlValueLiteral::String(value) => serde_json::from_str(value).map_err(|e| {
                        anyhow!("Column '{}' expects valid JSON/JSONB: {}", column_name, e)
                    })?,
                    SqlValueLiteral::Null if column.nullable => serde_json::Value::Null,
                    _ => self.literal_to_json(val)?,
                };
                if column.properties.get("json_encoding").map(String::as_str) == Some("jsonb") {
                    Ok(ProximaValue::Jsonb(json))
                } else {
                    Ok(ProximaValue::Json(json))
                }
            }
            CatalogDataType::Vector => self.literal_to_vector(val).map(ProximaValue::DenseVector),
            CatalogDataType::Binary | CatalogDataType::BinaryVector => match val {
                SqlValueLiteral::Binary(value) => Ok(ProximaValue::Binary(value.clone())),
                SqlValueLiteral::Null if column.nullable => Ok(ProximaValue::Null),
                _ => Err(anyhow!("Column '{}' expects binary", column_name)),
            },
            CatalogDataType::Date => self
                .literal_to_i64(val)
                .map(|value| ProximaValue::Date(value as i32)),
            CatalogDataType::Time => self.literal_to_i64(val).map(|value| {
                ProximaValue::Time(value, proximadb_data_model::TimeUnit::Millisecond)
            }),
            CatalogDataType::Timestamp => self.literal_to_timestamp(val).map(|value| {
                value
                    .map(|timestamp| {
                        ProximaValue::Timestamp(
                            timestamp,
                            proximadb_data_model::TimeUnit::Millisecond,
                        )
                    })
                    .unwrap_or(ProximaValue::Null)
            }),
            CatalogDataType::TimestampTz => self.literal_to_timestamp(val).map(|value| {
                value
                    .map(|timestamp| {
                        ProximaValue::TimestampTz(
                            timestamp,
                            proximadb_data_model::TimeUnit::Millisecond,
                        )
                    })
                    .unwrap_or(ProximaValue::Null)
            }),
            CatalogDataType::Decimal => self.literal_to_string(val).map(ProximaValue::Decimal),
            CatalogDataType::SparseVector => Err(anyhow!(
                "Sparse vector DML literal lowering is not implemented for column '{}'",
                column_name
            )),
        }
    }

    fn literal_to_proxima_value(&self, val: &SqlValueLiteral) -> Result<ProximaValue> {
        match val {
            SqlValueLiteral::Null => Ok(ProximaValue::Null),
            SqlValueLiteral::Boolean(value) => Ok(ProximaValue::Boolean(*value)),
            SqlValueLiteral::Integer(value) => Ok(ProximaValue::Int64(*value)),
            SqlValueLiteral::Float(value) => Ok(ProximaValue::Float64(*value)),
            SqlValueLiteral::String(value) => Ok(ProximaValue::String(value.clone())),
            SqlValueLiteral::Binary(value) => Ok(ProximaValue::Binary(value.clone())),
            SqlValueLiteral::Json(value) => Ok(ProximaValue::Json(value.clone())),
            SqlValueLiteral::Array(values) => values
                .iter()
                .map(|value| self.literal_to_proxima_value(value))
                .collect::<Result<Vec<_>>>()
                .map(ProximaValue::Array),
            SqlValueLiteral::Function { name, .. }
                if name.eq_ignore_ascii_case("NOW")
                    || name.eq_ignore_ascii_case("CURRENT_TIMESTAMP") =>
            {
                Ok(ProximaValue::TimestampTz(
                    chrono::Utc::now().timestamp_millis(),
                    proximadb_data_model::TimeUnit::Millisecond,
                ))
            }
            SqlValueLiteral::Default => Err(anyhow!("DEFAULT value is not supported yet")),
            SqlValueLiteral::Parameter(_) => {
                Err(anyhow!("Unbound parameter in literal conversion"))
            }
            SqlValueLiteral::Column(_) => {
                Err(anyhow!("Column reference not supported in value context"))
            }
            SqlValueLiteral::Function { name, .. } => Err(anyhow!("Unsupported function: {name}")),
        }
    }

    fn literal_to_i64(&self, val: &SqlValueLiteral) -> Result<i64> {
        match val {
            SqlValueLiteral::Integer(value) => Ok(*value),
            SqlValueLiteral::String(value) => value
                .parse()
                .map_err(|e| anyhow!("Invalid integer literal '{}': {}", value, e)),
            SqlValueLiteral::Null => Err(anyhow!("Cannot convert NULL to integer")),
            _ => Err(anyhow!("Expected integer literal")),
        }
    }

    fn literal_to_f64(&self, val: &SqlValueLiteral) -> Result<f64> {
        match val {
            SqlValueLiteral::Float(value) => Ok(*value),
            SqlValueLiteral::Integer(value) => Ok(*value as f64),
            SqlValueLiteral::String(value) => value
                .parse()
                .map_err(|e| anyhow!("Invalid float literal '{}': {}", value, e)),
            SqlValueLiteral::Null => Err(anyhow!("Cannot convert NULL to float")),
            _ => Err(anyhow!("Expected numeric literal")),
        }
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
    use proximadb_catalog::CatalogColumn;

    fn update_test_schema() -> CatalogTableSchema {
        CatalogTableSchema::new("agent_store")
            .with_column(
                CatalogColumn::new(1, "record_id", CatalogDataType::String).nullable(false),
            )
            .with_column(CatalogColumn::new(2, "name", CatalogDataType::String).nullable(false))
            .with_column(
                CatalogColumn::new(3, "payload", CatalogDataType::Json).with_default("'{}'::jsonb"),
            )
            .with_column(CatalogColumn::new(4, "notes", CatalogDataType::String))
    }

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

    #[test]
    fn test_parse_jsonb_default_literal() {
        let literal = DmlService::parse_default_literal("'{}'::jsonb").unwrap();
        match literal {
            SqlValueLiteral::Json(value) => {
                assert_eq!(value, serde_json::json!({}));
            }
            other => panic!("expected JSON default literal, got {other:?}"),
        }
    }

    #[test]
    fn test_parse_default_literal_unescapes_sql_string() {
        let literal = DmlService::parse_default_literal("'agent''s note'").unwrap();
        match literal {
            SqlValueLiteral::String(value) => {
                assert_eq!(value, "agent's note");
            }
            other => panic!("expected string default literal, got {other:?}"),
        }
    }

    #[test]
    fn test_update_assignment_validation_rejects_primary_key_change() {
        let err = DmlService::validate_update_assignments(
            &[(
                "record_id".to_string(),
                SqlValueLiteral::String("r2".to_string()),
            )],
            &update_test_schema(),
        )
        .unwrap_err();

        assert!(err.to_string().contains("cannot modify primary key"));
    }

    #[test]
    fn test_update_assignment_validation_rejects_null_for_not_null_column() {
        let err = DmlService::validate_update_assignments(
            &[("name".to_string(), SqlValueLiteral::Null)],
            &update_test_schema(),
        )
        .unwrap_err();

        assert!(err.to_string().contains("cannot be NULL"));
    }

    #[test]
    fn test_update_assignment_validation_accepts_default_with_catalog_default() {
        DmlService::validate_update_assignments(
            &[("payload".to_string(), SqlValueLiteral::Default)],
            &update_test_schema(),
        )
        .unwrap();
    }

    #[test]
    fn test_update_assignment_validation_rejects_default_without_catalog_default() {
        let err = DmlService::validate_update_assignments(
            &[("notes".to_string(), SqlValueLiteral::Default)],
            &update_test_schema(),
        )
        .unwrap_err();

        assert!(err.to_string().contains("has no DEFAULT"));
    }
}
