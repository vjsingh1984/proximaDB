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
    CatalogColumn, CatalogDataType, CatalogStorageLayout, CatalogTableSchema,
    CatalogTableStatistics,
    relational::{
        CatalogRow, RelationalMutationKind, RelationalRecordOptions, RelationalWriteProfile,
    },
};
use proximadb_data_model::ProximaValue;
use proximadb_records::{EmbeddingCell, ProximaRecord, ProximaTreeNode, RecordStorage};
use tracing::{debug, info};

use crate::catalog::CatalogManager;
use crate::query::table_write_executor::{
    NativeTableWriteExecutor, PlannedOnlyTableWriteExecutor, TableRecordStoreSourceReader,
    TableWriteExecutionRequest, TableWriteExecutionStatus, TableWriteExecutor,
};
use crate::query::table_write_plan::{
    CopyIntoPlan, DmlWritePlanRequest, DmlWritePlanner, ReadSource, RoutedExecutionPlan,
    TableWriteRouteExplanation, WriteIntentOverrides,
};
use crate::services::operations::VectorOps;
use crate::services::operations::vectors::RichSearchResult;
use crate::services::record_store::{
    CatalogRoutingTableRecordStore, DirectWalTableRecordStore, TableRecordGetRequest,
    TableRecordMutation, TableRecordMutationKind, TableRecordScanRequest, TableRecordStore,
    TableWalAppender, VectorOpsTableRecordStore,
};
use crate::services::{
    WriteDurabilityRequirement, WriteIntent, WriteLaneDecision, WriteLaneRouter, WriteOperationKind,
};

/// Comparison operators supported by the lightweight catalog-table SELECT path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationalSelectPredicateOperator {
    Equal,
    NotEqual,
    LessThan,
    LessThanOrEqual,
    GreaterThan,
    GreaterThanOrEqual,
}

/// Catalog-shaped predicate condition for simple relational scans.
#[derive(Debug, Clone)]
pub enum RelationalSelectPredicateCondition {
    Comparison {
        operator: RelationalSelectPredicateOperator,
        literal: String,
    },
    In {
        literals: Vec<String>,
        negated: bool,
    },
    Like {
        pattern: String,
        negated: bool,
    },
    IsNull {
        negated: bool,
    },
}

/// Catalog-resolved predicate used by simple pgwire relational scans.
#[derive(Debug, Clone)]
pub struct RelationalSelectPredicate {
    pub column: proximadb_catalog::CatalogColumn,
    pub condition: RelationalSelectPredicateCondition,
}

/// Syntax-level predicate input from a SQL/protocol facade.
///
/// DML resolves this against xCatalog before route selection or evaluation.
#[derive(Debug, Clone)]
pub struct RelationalSelectPredicateInput {
    pub column_name: String,
    pub condition: RelationalSelectPredicateCondition,
}

/// Read access path selected for a simple relational SELECT.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelationalSelectAccessPath {
    PrimaryKeyLookup,
    TableScan,
}

/// Catalog and route metadata for a simple relational SELECT.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelationalSelectRouteMetadata {
    pub access_path: RelationalSelectAccessPath,
    pub authority_mode: String,
    pub workload_profile: String,
    pub storage_specialization: String,
    pub policy_boundary: String,
    pub predicate_count: usize,
    pub projected_column_count: usize,
    pub limit: Option<usize>,
}

/// Catalog-resolved result for a simple relational SELECT.
#[derive(Debug, Clone)]
pub struct RelationalSelectResult {
    pub selected_columns: Vec<CatalogColumn>,
    pub route_metadata: RelationalSelectRouteMetadata,
    pub rows: Vec<Vec<ProximaValue>>,
}

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
    /// INSERT INTO target SELECT ... FROM source.
    InsertSelect {
        /// Logical copy/write plan for the statement.
        plan: CopyIntoPlan,
        /// Optional target column list.
        columns: Vec<String>,
    },
    /// INSERT OVERWRITE target SELECT ... FROM source.
    InsertOverwrite {
        /// Logical overwrite plan for the statement.
        plan: CopyIntoPlan,
        /// Optional target column list.
        columns: Vec<String>,
    },
}

impl DmlStatement {
    /// Return the logical target table for all DML variants.
    pub fn target_table_name(&self) -> &str {
        match self {
            Self::Insert { table_name, .. }
            | Self::Update { table_name, .. }
            | Self::Delete { table_name, .. }
            | Self::Upsert { table_name, .. } => table_name,
            Self::InsertSelect { plan, .. } | Self::InsertOverwrite { plan, .. } => {
                &plan.target.name
            }
        }
    }
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
    /// Canonical table-record store for DML mutations.
    record_store: Arc<dyn TableRecordStore>,
    /// Routed table-write executor for INSERT SELECT / OVERWRITE / CTAS / MERGE.
    table_write_executor: Arc<dyn TableWriteExecutor>,
}

impl DmlService {
    /// Create a new DML service
    pub fn new(catalog_manager: Arc<CatalogManager>, vector_ops: Arc<VectorOps>) -> Self {
        Self::with_record_store(
            catalog_manager,
            Arc::new(CatalogRoutingTableRecordStore::with_vector_compatibility(
                vector_ops,
            )),
        )
    }

    /// Create a DML service over an explicit canonical table-record store.
    pub fn with_record_store(
        catalog_manager: Arc<CatalogManager>,
        record_store: Arc<dyn TableRecordStore>,
    ) -> Self {
        Self::with_record_store_and_table_write_executor(
            catalog_manager,
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        )
    }

    /// Create a DML service with the target direct canonical writer enabled.
    ///
    /// Relational/PAX/OLTP/OLAP/HTAP tables route through
    /// `DirectWalTableRecordStore` into canonical WAL plus `RecordStorage`.
    /// Legacy vector/LSM-specialized tables still route through the VectorOps
    /// compatibility adapter until those formats are retired or migrated.
    pub fn with_direct_record_storage(
        catalog_manager: Arc<CatalogManager>,
        vector_ops: Arc<VectorOps>,
        record_storage: Arc<dyn RecordStorage>,
        wal_appender: Arc<dyn TableWalAppender>,
    ) -> Self {
        let canonical_store =
            Arc::new(DirectWalTableRecordStore::new(record_storage, wal_appender));
        let legacy_store = Arc::new(VectorOpsTableRecordStore::new(vector_ops));
        let routed_store: Arc<dyn TableRecordStore> = Arc::new(
            CatalogRoutingTableRecordStore::new(canonical_store, legacy_store),
        );
        let source_reader = Arc::new(TableRecordStoreSourceReader::new(routed_store.clone()));
        let table_write_executor = Arc::new(NativeTableWriteExecutor::new(
            source_reader,
            routed_store.clone(),
        ));

        Self::with_record_store_and_table_write_executor(
            catalog_manager,
            routed_store,
            table_write_executor,
        )
    }

    /// Create a DML service over explicit DML and table-write executors.
    pub fn with_record_store_and_table_write_executor(
        catalog_manager: Arc<CatalogManager>,
        record_store: Arc<dyn TableRecordStore>,
        table_write_executor: Arc<dyn TableWriteExecutor>,
    ) -> Self {
        Self {
            catalog_manager,
            record_store,
            table_write_executor,
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
            DmlStatement::InsertSelect { plan, columns }
            | DmlStatement::InsertOverwrite { plan, columns } => {
                self.plan_table_write(&plan, &columns).await?
            }
        };

        Ok(result.with_execution_time(start.elapsed().as_micros() as u64))
    }

    /// Scan current visible records for a cataloged table through the shared
    /// table-record store boundary.
    pub async fn scan_table_records(
        &self,
        table_name: &str,
        limit: Option<usize>,
    ) -> Result<(CatalogTableSchema, Vec<ProximaRecord>)> {
        self.scan_table_records_with_predicates(table_name, limit, &[])
            .await
    }

    /// Scan current visible records and apply simple catalog-shaped predicates
    /// behind the DML/catalog boundary.
    pub async fn scan_table_records_with_predicates(
        &self,
        table_name: &str,
        limit: Option<usize>,
        predicates: &[RelationalSelectPredicateInput],
    ) -> Result<(CatalogTableSchema, Vec<ProximaRecord>)> {
        self.select_table_records(table_name, limit, predicates)
            .await
    }

    /// Select current visible records and resolve projection columns inside the
    /// DML/catalog boundary.
    pub async fn select_table_records_with_projection(
        &self,
        table_name: &str,
        projection_column_names: &[String],
        limit: Option<usize>,
        predicates: &[RelationalSelectPredicateInput],
    ) -> Result<RelationalSelectResult> {
        let (table_schema, table_id_name) = self.resolve_select_table(table_name).await?;
        let selected_columns =
            Self::resolve_select_projection(&table_schema, projection_column_names)?;
        let predicates = Self::resolve_select_predicates(&table_schema, predicates)?;
        let (access_path, records) = self
            .select_table_records_with_resolved_predicates(
                &table_schema,
                &table_id_name,
                limit,
                &predicates,
            )
            .await?;
        let rows = Self::project_select_rows(&records, &table_schema, &selected_columns)?;
        let route_metadata = Self::select_route_metadata(
            &table_schema,
            &selected_columns,
            &predicates,
            limit,
            access_path,
        );

        Ok(RelationalSelectResult {
            selected_columns,
            route_metadata,
            rows,
        })
    }

    /// Select current visible records for simple catalog-table reads.
    ///
    /// This is the relational read routing boundary for pgwire's current
    /// compatibility SELECT path: it chooses point lookup when catalog-shaped
    /// predicates contain primary-key equality, otherwise it uses the
    /// table-record scan contract.
    pub async fn select_table_records(
        &self,
        table_name: &str,
        limit: Option<usize>,
        predicates: &[RelationalSelectPredicateInput],
    ) -> Result<(CatalogTableSchema, Vec<ProximaRecord>)> {
        let (table_schema, table_id_name) = self.resolve_select_table(table_name).await?;
        let predicates = Self::resolve_select_predicates(&table_schema, predicates)?;
        let (_, records) = self
            .select_table_records_with_resolved_predicates(
                &table_schema,
                &table_id_name,
                limit,
                &predicates,
            )
            .await?;
        Ok((table_schema, records))
    }

    async fn resolve_select_table(&self, table_name: &str) -> Result<(CatalogTableSchema, String)> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!("Table '{table_name}' does not exist"));
        }

        Ok((catalog.get_table(&table_id).await?, table_id.name.clone()))
    }

    async fn select_table_records_with_resolved_predicates(
        &self,
        table_schema: &CatalogTableSchema,
        table_id_name: &str,
        limit: Option<usize>,
        predicates: &[RelationalSelectPredicate],
    ) -> Result<(RelationalSelectAccessPath, Vec<ProximaRecord>)> {
        if let Some(primary_key_value) = Self::primary_key_lookup_value(table_schema, predicates) {
            let record = self
                .record_store
                .get_by_key(
                    table_schema,
                    TableRecordGetRequest {
                        table_id: table_id_name.to_string(),
                        key: primary_key_value,
                        include_vector: true,
                        include_props: true,
                    },
                    None,
                )
                .await?;
            let primary_key = table_schema.primary_key.first().map(String::as_str);
            return Ok((
                RelationalSelectAccessPath::PrimaryKeyLookup,
                record
                    .map(Self::rich_result_to_record)
                    .filter(|record| {
                        Self::record_matches_select_predicates(record, &predicates, primary_key)
                    })
                    .into_iter()
                    .take(limit.unwrap_or(usize::MAX))
                    .collect(),
            ));
        }

        let scan_limit = if predicates.is_empty() { limit } else { None };
        let mut records = self
            .record_store
            .scan_records(
                table_schema,
                TableRecordScanRequest {
                    table_id: table_id_name.to_string(),
                    limit: scan_limit,
                    include_vector: true,
                    include_props: true,
                },
                None,
            )
            .await?;
        if !predicates.is_empty() {
            let primary_key = table_schema.primary_key.first().map(String::as_str);
            let mut filtered = Vec::new();
            for record in records {
                if Self::record_matches_select_predicates(&record, &predicates, primary_key) {
                    filtered.push(record);
                    if limit.is_some_and(|limit| filtered.len() >= limit) {
                        break;
                    }
                }
            }
            records = filtered;
        }

        Ok((RelationalSelectAccessPath::TableScan, records))
    }

    /// Get the current visible record for a cataloged table through the shared
    /// table-record store boundary.
    pub async fn get_table_record(
        &self,
        table_name: &str,
        key: &str,
        include_vector: bool,
        include_props: bool,
    ) -> Result<(CatalogTableSchema, Option<RichSearchResult>)> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!("Table '{table_name}' does not exist"));
        }

        let table_schema = catalog.get_table(&table_id).await?;
        let record = self
            .record_store
            .get_by_key(
                &table_schema,
                TableRecordGetRequest {
                    table_id: table_id.name.clone(),
                    key: key.to_string(),
                    include_vector,
                    include_props,
                },
                None,
            )
            .await?;

        Ok((table_schema, record))
    }

    /// Explain table-to-table write routing without executing the write.
    pub async fn explain_table_write(
        &self,
        statement: DmlStatement,
    ) -> Result<TableWriteRouteExplanation> {
        self.explain_table_write_with_overrides(statement, None)
            .await
    }

    /// Explain table-to-table write routing with protocol/session write intent hints.
    pub async fn explain_table_write_with_overrides(
        &self,
        statement: DmlStatement,
        write_intent_overrides: Option<&WriteIntentOverrides>,
    ) -> Result<TableWriteRouteExplanation> {
        match statement {
            DmlStatement::InsertSelect { plan, columns }
            | DmlStatement::InsertOverwrite { plan, columns } => {
                let routed = self
                    .route_table_write_plan_with_overrides(&plan, &columns, write_intent_overrides)
                    .await?;
                Ok(routed.route_explanation())
            }
            other => Err(anyhow!(
                "EXPLAIN table-write routing only supports INSERT ... SELECT and INSERT OVERWRITE; got {:?}",
                other
            )),
        }
    }

    /// Resolve catalog metadata and route table-to-table writes before execution.
    async fn plan_table_write(
        &self,
        plan: &CopyIntoPlan,
        target_columns: &[String],
    ) -> Result<DmlResult> {
        let (table_schema, target_stats) = self
            .resolve_table_metadata(&plan.target.qualified_name())
            .await?;
        let source_metadata = self.resolve_table_write_source_metadata(plan).await?;
        let source_schema = source_metadata.as_ref().map(|(schema, _)| schema);
        let source_stats = source_metadata.as_ref().map(|(_, stats)| stats);
        let routed = self.route_table_write_with_schemas(
            plan,
            target_columns,
            &table_schema,
            Some(&target_stats),
            source_schema,
            source_stats,
            None,
        )?;
        let execution = self
            .table_write_executor
            .execute(TableWriteExecutionRequest {
                target_schema: &table_schema,
                source_schema,
                routed_plan: routed,
            })
            .await?;

        match execution.status {
            TableWriteExecutionStatus::Completed => Ok(DmlResult::success(
                execution.rows_written,
                format!("Table write completed through {}", execution.route_summary),
            )),
            TableWriteExecutionStatus::PlannedOnly => Err(anyhow!(
                "INSERT ... SELECT and INSERT OVERWRITE execution is not implemented yet; planned route: {}, guards={:?}",
                execution.route_summary,
                execution.guards
            )),
        }
    }

    async fn route_table_write_plan_with_overrides(
        &self,
        plan: &CopyIntoPlan,
        target_columns: &[String],
        write_intent_overrides: Option<&WriteIntentOverrides>,
    ) -> Result<RoutedExecutionPlan> {
        let target_table_name = plan.target.qualified_name();
        let (table_schema, target_stats) = self.resolve_table_metadata(&target_table_name).await?;
        let source_metadata = self.resolve_table_write_source_metadata(plan).await?;
        let source_schema = source_metadata.as_ref().map(|(schema, _)| schema);
        let source_stats = source_metadata.as_ref().map(|(_, stats)| stats);
        self.route_table_write_with_schemas(
            plan,
            target_columns,
            &table_schema,
            Some(&target_stats),
            source_schema,
            source_stats,
            write_intent_overrides,
        )
    }

    async fn resolve_table_metadata(
        &self,
        table_name: &str,
    ) -> Result<(CatalogTableSchema, CatalogTableStatistics)> {
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;

        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!("Table '{table_name}' does not exist"));
        }

        let schema = catalog.get_table(&table_id).await?;
        let stats = catalog.get_statistics(&table_id).await.unwrap_or_default();
        Ok((schema, stats))
    }

    fn route_table_write_with_schemas(
        &self,
        plan: &CopyIntoPlan,
        target_columns: &[String],
        target_schema: &CatalogTableSchema,
        target_stats: Option<&CatalogTableStatistics>,
        source_schema: Option<&CatalogTableSchema>,
        source_stats: Option<&CatalogTableStatistics>,
        write_intent_overrides: Option<&WriteIntentOverrides>,
    ) -> Result<RoutedExecutionPlan> {
        DmlWritePlanner::default().plan(DmlWritePlanRequest {
            target_schema,
            target_stats,
            source_schema,
            source_stats,
            write_intent_overrides,
            plan,
            target_columns,
        })
    }

    fn route_row_dml_write_intent(
        table_schema: &CatalogTableSchema,
        operation_kind: WriteOperationKind,
        row_count: usize,
    ) -> (WriteIntent, WriteLaneDecision) {
        let mut intent = WriteIntent::new(table_schema.name.clone(), operation_kind)
            .with_durability(WriteDurabilityRequirement::WalRequired)
            .with_row_count_hint(row_count as u64);

        if operation_kind.requires_row_level_mvcc() {
            intent = intent.with_row_level_semantics(true);
        }

        let decision = WriteLaneRouter::new().route(&intent);
        (intent, decision)
    }

    fn trace_row_dml_write_lane(intent: &WriteIntent, decision: &WriteLaneDecision) {
        debug!(
            table = %intent.target_table,
            operation = ?intent.operation_kind,
            durability = ?intent.durability,
            write_lane = ?decision.lane,
            guards = ?decision.required_guards,
            rejected_lanes = ?decision.rejected_lanes,
            "Routed row-level DML write intent"
        );
    }

    fn resolve_select_predicates(
        table_schema: &CatalogTableSchema,
        predicates: &[RelationalSelectPredicateInput],
    ) -> Result<Vec<RelationalSelectPredicate>> {
        predicates
            .iter()
            .map(|predicate| {
                let column = table_schema
                    .columns
                    .iter()
                    .find(|column| column.name.eq_ignore_ascii_case(&predicate.column_name))
                    .cloned()
                    .ok_or_else(|| {
                        anyhow!(
                            "Column '{}' does not exist in table '{}'",
                            predicate.column_name,
                            table_schema.name
                        )
                    })?;
                Ok(RelationalSelectPredicate {
                    column,
                    condition: predicate.condition.clone(),
                })
            })
            .collect()
    }

    fn resolve_select_projection(
        table_schema: &CatalogTableSchema,
        projection_column_names: &[String],
    ) -> Result<Vec<CatalogColumn>> {
        if projection_column_names.is_empty() {
            return Ok(table_schema.columns.clone());
        }

        projection_column_names
            .iter()
            .map(|column_name| {
                table_schema
                    .columns
                    .iter()
                    .find(|column| column.name.eq_ignore_ascii_case(column_name))
                    .cloned()
                    .ok_or_else(|| {
                        anyhow!(
                            "Column '{}' does not exist in table '{}'",
                            column_name,
                            table_schema.name
                        )
                    })
            })
            .collect()
    }

    fn select_route_metadata(
        table_schema: &CatalogTableSchema,
        selected_columns: &[CatalogColumn],
        predicates: &[RelationalSelectPredicate],
        limit: Option<usize>,
        access_path: RelationalSelectAccessPath,
    ) -> RelationalSelectRouteMetadata {
        RelationalSelectRouteMetadata {
            access_path,
            authority_mode: Self::select_authority_mode(table_schema),
            workload_profile: table_schema.workload_profile.as_str().to_string(),
            storage_specialization: table_schema.storage_specialization.as_str().to_string(),
            policy_boundary: Self::select_policy_boundary(table_schema),
            predicate_count: predicates.len(),
            projected_column_count: selected_columns.len(),
            limit,
        }
    }

    fn select_authority_mode(table_schema: &CatalogTableSchema) -> String {
        Self::select_primary_layout(table_schema)
            .map(|layout| layout.authority.ownership_mode_name().to_string())
            .unwrap_or_else(|| "ProximaAuthoritative".to_string())
    }

    fn select_policy_boundary(table_schema: &CatalogTableSchema) -> String {
        if let Some(boundary) = table_schema
            .properties
            .get("policy_boundary")
            .or_else(|| table_schema.properties.get("rls_boundary"))
        {
            return boundary.clone();
        }

        match Self::select_primary_layout(table_schema) {
            Some(layout) if layout.policy_enforced_in_proxima => "engine-enforced".to_string(),
            Some(layout) if layout.authority.is_external_authoritative() => {
                "external-policy".to_string()
            }
            Some(_) => "unsupported".to_string(),
            None => "engine-enforced".to_string(),
        }
    }

    fn select_primary_layout(table_schema: &CatalogTableSchema) -> Option<&CatalogStorageLayout> {
        table_schema
            .storage_layouts
            .iter()
            .rev()
            .find(|layout| layout.name == "primary")
            .or_else(|| table_schema.storage_layouts.first())
    }

    fn primary_key_lookup_value(
        table_schema: &CatalogTableSchema,
        predicates: &[RelationalSelectPredicate],
    ) -> Option<String> {
        let primary_key = table_schema.primary_key.first()?;
        predicates.iter().find_map(|predicate| {
            if !predicate.column.name.eq_ignore_ascii_case(primary_key) {
                return None;
            }
            let RelationalSelectPredicateCondition::Comparison {
                operator: RelationalSelectPredicateOperator::Equal,
                literal,
            } = &predicate.condition
            else {
                return None;
            };
            Some(literal.clone())
        })
    }

    fn rich_result_to_record(result: RichSearchResult) -> ProximaRecord {
        let embeddings = if result.vector.is_empty() {
            Vec::new()
        } else {
            vec![EmbeddingCell {
                model_id: "default".to_string(),
                modality: "vector".to_string(),
                dim: result.vector.len() as u32,
                values: result.vector,
            }]
        };

        ProximaRecord {
            oid: result.id.clone(),
            local_id: Some(result.id),
            props: result
                .props
                .into_iter()
                .map(|(key, value)| (key, ProximaTreeNode::Value(value)))
                .collect(),
            embeddings,
            record_version: result.version.map(u64::from).unwrap_or_default(),
            created_at_ns: result
                .timestamp
                .map(|timestamp_ms| timestamp_ms.saturating_mul(1_000_000))
                .unwrap_or_default(),
            origin: result.source,
            ..ProximaRecord::default()
        }
    }

    fn project_select_rows(
        records: &[ProximaRecord],
        table_schema: &CatalogTableSchema,
        selected_columns: &[CatalogColumn],
    ) -> Result<Vec<Vec<ProximaValue>>> {
        let primary_key = table_schema.primary_key.first().map(String::as_str);
        records
            .iter()
            .map(|record| {
                selected_columns
                    .iter()
                    .map(|column| {
                        Self::record_column_value_for_select(
                            record,
                            column,
                            primary_key,
                            table_schema,
                        )
                    })
                    .collect()
            })
            .collect()
    }

    fn record_column_value_for_select(
        record: &ProximaRecord,
        column: &CatalogColumn,
        primary_key: Option<&str>,
        table_schema: &CatalogTableSchema,
    ) -> Result<ProximaValue> {
        if let Some(proximadb_records::ProximaTreeNode::Value(value)) =
            record.props.get(&column.name)
        {
            return Ok(value.clone());
        }
        if primary_key.is_some_and(|primary_key| column.name.eq_ignore_ascii_case(primary_key)) {
            return Self::primary_key_string_to_proxima_value(
                &column.name,
                &record.oid,
                table_schema,
            );
        }
        if matches!(
            column.data_type,
            CatalogDataType::Vector | CatalogDataType::SparseVector | CatalogDataType::BinaryVector
        ) && let Some(embedding) = record.embeddings.first()
        {
            return Ok(ProximaValue::DenseVector(embedding.values.clone()));
        }
        Ok(ProximaValue::Null)
    }

    /// Evaluate simple catalog-shaped predicates against a canonical record.
    pub fn record_matches_select_predicates(
        record: &ProximaRecord,
        predicates: &[RelationalSelectPredicate],
        primary_key: Option<&str>,
    ) -> bool {
        predicates.iter().all(|predicate| {
            let value =
                Self::record_column_value_for_predicate(record, &predicate.column, primary_key);
            match &predicate.condition {
                RelationalSelectPredicateCondition::Comparison { operator, literal } => {
                    Self::compare_catalog_value(
                        &value,
                        literal,
                        *operator,
                        predicate.column.data_type,
                    )
                }
                RelationalSelectPredicateCondition::In { literals, negated } => {
                    let matches = literals.iter().any(|literal| {
                        Self::compare_catalog_value(
                            &value,
                            literal,
                            RelationalSelectPredicateOperator::Equal,
                            predicate.column.data_type,
                        )
                    });
                    if *negated { !matches } else { matches }
                }
                RelationalSelectPredicateCondition::Like { pattern, negated } => {
                    let matches = Self::sql_like_matches(&value, pattern);
                    if *negated { !matches } else { matches }
                }
                RelationalSelectPredicateCondition::IsNull { negated } => {
                    let matches = value.is_empty();
                    if *negated { !matches } else { matches }
                }
            }
        })
    }

    /// Resolve and evaluate syntax-level predicate inputs for tests and simple
    /// read adapters that already have a catalog schema.
    pub fn record_matches_select_predicate_inputs(
        record: &ProximaRecord,
        table_schema: &CatalogTableSchema,
        predicates: &[RelationalSelectPredicateInput],
    ) -> Result<bool> {
        let predicates = Self::resolve_select_predicates(table_schema, predicates)?;
        Ok(Self::record_matches_select_predicates(
            record,
            &predicates,
            table_schema.primary_key.first().map(String::as_str),
        ))
    }

    fn record_column_value_for_predicate(
        record: &ProximaRecord,
        column: &proximadb_catalog::CatalogColumn,
        primary_key: Option<&str>,
    ) -> String {
        if let Some(proximadb_records::ProximaTreeNode::Value(value)) =
            record.props.get(&column.name)
        {
            return Self::proxima_value_to_predicate_text(value);
        }
        if primary_key.is_some_and(|primary_key| column.name.eq_ignore_ascii_case(primary_key)) {
            return record.oid.clone();
        }
        if matches!(
            column.data_type,
            CatalogDataType::Vector | CatalogDataType::SparseVector | CatalogDataType::BinaryVector
        ) && let Some(embedding) = record.embeddings.first()
        {
            return Self::proxima_value_to_predicate_text(&ProximaValue::DenseVector(
                embedding.values.clone(),
            ));
        }
        String::new()
    }

    fn proxima_value_to_predicate_text(value: &ProximaValue) -> String {
        match value {
            ProximaValue::Boolean(value) => {
                if *value {
                    "t".to_string()
                } else {
                    "f".to_string()
                }
            }
            ProximaValue::Int8(value) => value.to_string(),
            ProximaValue::Int16(value) => value.to_string(),
            ProximaValue::Int32(value) => value.to_string(),
            ProximaValue::Int64(value) => value.to_string(),
            ProximaValue::UInt8(value) => value.to_string(),
            ProximaValue::UInt16(value) => value.to_string(),
            ProximaValue::UInt32(value) => value.to_string(),
            ProximaValue::UInt64(value) => value.to_string(),
            ProximaValue::Float16(value) => value.to_string(),
            ProximaValue::Float32(value) => value.to_string(),
            ProximaValue::Float64(value) => value.to_string(),
            ProximaValue::Decimal(value) => value.clone(),
            ProximaValue::String(value) | ProximaValue::Symbol(value) => value.clone(),
            ProximaValue::DenseVector(values) => values
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(","),
            ProximaValue::Null => String::new(),
            other => format!("{other:?}"),
        }
    }

    fn compare_catalog_value(
        value: &str,
        literal: &str,
        operator: RelationalSelectPredicateOperator,
        data_type: CatalogDataType,
    ) -> bool {
        match data_type {
            CatalogDataType::Boolean => {
                let left = Self::normalize_bool_literal(value);
                let right = Self::normalize_bool_literal(literal);
                Self::compare_ordered_values(left, right, operator)
            }
            CatalogDataType::Int8
            | CatalogDataType::Int16
            | CatalogDataType::Int32
            | CatalogDataType::Int64
            | CatalogDataType::Float32
            | CatalogDataType::Float64
            | CatalogDataType::Decimal => {
                let Ok(left) = value.parse::<f64>() else {
                    return false;
                };
                let Ok(right) = literal.parse::<f64>() else {
                    return false;
                };
                match operator {
                    RelationalSelectPredicateOperator::Equal => (left - right).abs() < f64::EPSILON,
                    RelationalSelectPredicateOperator::NotEqual => {
                        (left - right).abs() >= f64::EPSILON
                    }
                    RelationalSelectPredicateOperator::LessThan => left < right,
                    RelationalSelectPredicateOperator::LessThanOrEqual => left <= right,
                    RelationalSelectPredicateOperator::GreaterThan => left > right,
                    RelationalSelectPredicateOperator::GreaterThanOrEqual => left >= right,
                }
            }
            _ => Self::compare_ordered_values(value.to_string(), literal.to_string(), operator),
        }
    }

    fn normalize_bool_literal(value: &str) -> String {
        match value.trim().to_ascii_lowercase().as_str() {
            "t" | "true" | "1" | "yes" | "on" => "t".to_string(),
            "f" | "false" | "0" | "no" | "off" => "f".to_string(),
            other => other.to_string(),
        }
    }

    fn compare_ordered_values(
        left: String,
        right: String,
        operator: RelationalSelectPredicateOperator,
    ) -> bool {
        match operator {
            RelationalSelectPredicateOperator::Equal => left == right,
            RelationalSelectPredicateOperator::NotEqual => left != right,
            RelationalSelectPredicateOperator::LessThan => left < right,
            RelationalSelectPredicateOperator::LessThanOrEqual => left <= right,
            RelationalSelectPredicateOperator::GreaterThan => left > right,
            RelationalSelectPredicateOperator::GreaterThanOrEqual => left >= right,
        }
    }

    fn sql_like_matches(value: &str, pattern: &str) -> bool {
        let value = value.chars().collect::<Vec<_>>();
        let pattern = pattern.chars().collect::<Vec<_>>();
        let mut table = vec![vec![false; pattern.len() + 1]; value.len() + 1];
        table[0][0] = true;

        for pattern_index in 1..=pattern.len() {
            if pattern[pattern_index - 1] == '%' {
                table[0][pattern_index] = table[0][pattern_index - 1];
            }
        }

        for value_index in 1..=value.len() {
            for pattern_index in 1..=pattern.len() {
                match pattern[pattern_index - 1] {
                    '%' => {
                        table[value_index][pattern_index] = table[value_index][pattern_index - 1]
                            || table[value_index - 1][pattern_index];
                    }
                    '_' => {
                        table[value_index][pattern_index] =
                            table[value_index - 1][pattern_index - 1];
                    }
                    ch => {
                        table[value_index][pattern_index] = ch == value[value_index - 1]
                            && table[value_index - 1][pattern_index - 1];
                    }
                }
            }
        }

        table[value.len()][pattern.len()]
    }

    async fn resolve_table_write_source_metadata(
        &self,
        plan: &CopyIntoPlan,
    ) -> Result<Option<(CatalogTableSchema, CatalogTableStatistics)>> {
        let ReadSource::CatalogTable { table, .. } = &plan.source else {
            return Ok(None);
        };

        let source_table_name = table.qualified_name();
        let (catalog, table_id) = self
            .catalog_manager
            .resolve_table(&source_table_name)
            .await?;
        if !catalog.table_exists(&table_id).await? {
            return Err(anyhow!(
                "Source table '{}' does not exist for table write into '{}'",
                source_table_name,
                plan.target.qualified_name()
            ));
        }

        let schema = catalog.get_table(&table_id).await?;
        let stats = catalog.get_statistics(&table_id).await.unwrap_or_default();
        Ok(Some((schema, stats)))
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
        let (write_intent, write_lane_decision) = Self::route_row_dml_write_intent(
            &table_schema,
            WriteOperationKind::Insert,
            values.len(),
        );
        Self::trace_row_dml_write_lane(&write_intent, &write_lane_decision);

        // Convert SQL literals into canonical ProximaRecord envelopes.
        let mut records = Vec::new();
        let mut inserted_ids = Vec::new();

        for row in values {
            let record = self.build_mutation_record(
                columns,
                &row,
                &table_schema,
                RelationalMutationKind::Insert,
            )?;
            inserted_ids.push(record.oid.clone());
            records.push(record);
        }

        // Insert canonical records through the table-record boundary. The current
        // implementation may adapt to legacy vector paths behind that trait.
        let num_records = records.len();
        let mutations = records
            .into_iter()
            .map(|record| TableRecordMutation::new(TableRecordMutationKind::Insert, record))
            .collect::<Vec<_>>();
        let batch_result = self
            .record_store
            .write_mutations(&table_schema, mutations, None)
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
        let (write_intent, write_lane_decision) = Self::route_row_dml_write_intent(
            &table_schema,
            WriteOperationKind::Update,
            ids_to_update.len(),
        );
        Self::trace_row_dml_write_lane(&write_intent, &write_lane_decision);
        Self::validate_update_assignments(&assignments, &table_schema)?;

        let mut records = Vec::new();
        let mut warnings = Vec::new();
        for record_id in &ids_to_update {
            let Some(existing) = self
                .record_store
                .get_by_key(
                    &table_schema,
                    TableRecordGetRequest {
                        table_id: table_id.name.clone(),
                        key: record_id.clone(),
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
        let mutations = records
            .into_iter()
            .map(|record| TableRecordMutation::new(TableRecordMutationKind::Update, record))
            .collect::<Vec<_>>();
        let batch_result = self
            .record_store
            .write_mutations(&table_schema, mutations, None)
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
        let (write_intent, write_lane_decision) = Self::route_row_dml_write_intent(
            &table_schema,
            WriteOperationKind::Delete,
            ids_to_delete.len(),
        );
        Self::trace_row_dml_write_lane(&write_intent, &write_lane_decision);

        let now_ns = chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0);
        let tombstones = ids_to_delete
            .iter()
            .map(|id| Self::build_delete_tombstone_record(id, &table_schema, now_ns))
            .collect::<Result<Vec<_>>>()?;
        let deleted_count = ids_to_delete.len();
        let mutations = tombstones
            .into_iter()
            .map(|record| TableRecordMutation::new(TableRecordMutationKind::Delete, record))
            .collect::<Vec<_>>();
        let batch_result = self
            .record_store
            .write_mutations(&table_schema, mutations, None)
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
        let (write_intent, write_lane_decision) = Self::route_row_dml_write_intent(
            &table_schema,
            WriteOperationKind::Upsert,
            values.len(),
        );
        Self::trace_row_dml_write_lane(&write_intent, &write_lane_decision);

        let mut records = Vec::new();
        let mut inserted_ids = Vec::new();

        for row in values {
            let record = self.build_mutation_record(
                columns,
                &row,
                &table_schema,
                RelationalMutationKind::Upsert,
            )?;
            inserted_ids.push(record.oid.clone());
            records.push(record);
        }

        let num_records = records.len();
        let mutations = records
            .into_iter()
            .map(|record| TableRecordMutation::new(TableRecordMutationKind::Upsert, record))
            .collect::<Vec<_>>();
        let batch_result = self
            .record_store
            .write_mutations(&table_schema, mutations, None)
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
    fn build_mutation_record(
        &self,
        columns: &[String],
        values: &[SqlValueLiteral],
        table_schema: &CatalogTableSchema,
        mutation_kind: RelationalMutationKind,
    ) -> Result<ProximaRecord> {
        if columns.len() != values.len() {
            return Err(anyhow!(
                "Column count ({}) doesn't match value count ({})",
                columns.len(),
                values.len()
            ));
        }

        self.validate_insert_columns(columns, values, table_schema)?;

        let mut row_values = HashMap::new();
        let mut created_at_ns = None;

        for (col, val) in columns.iter().zip(values.iter()) {
            let effective_value = self.effective_insert_literal(col, val, table_schema)?;
            let proxima_value =
                self.literal_to_proxima_value_for_column(col, &effective_value, table_schema)?;

            row_values.insert(col.clone(), proxima_value.clone());

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
        }

        let catalog_row =
            CatalogRow::validate(table_schema, row_values, &RelationalWriteProfile::oltp())?;
        let record = catalog_row.to_mutation_record(
            table_schema,
            mutation_kind,
            RelationalRecordOptions {
                method: Some(Self::mutation_method(mutation_kind).to_string()),
                created_at_ns,
                ..RelationalRecordOptions::default()
            },
        )?;
        Ok(record)
    }

    fn mutation_method(kind: RelationalMutationKind) -> &'static str {
        match kind {
            RelationalMutationKind::Insert => "sql_insert",
            RelationalMutationKind::Upsert => "sql_upsert",
            RelationalMutationKind::Update => "sql_update",
            RelationalMutationKind::Delete => "sql_delete",
        }
    }

    /// Build a canonical tombstone record for a primary-key-targeted DELETE.
    fn build_delete_tombstone_record(
        key_value: &str,
        table_schema: &CatalogTableSchema,
        now_ns: i64,
    ) -> Result<ProximaRecord> {
        let primary_key_column = Self::primary_key_column(table_schema).ok_or_else(|| {
            anyhow!(
                "Table '{}' has no single-column primary key/id column for DELETE",
                table_schema.name
            )
        })?;
        let key_proxima_value = Self::primary_key_string_to_proxima_value(
            &primary_key_column,
            key_value,
            table_schema,
        )?;
        let catalog_row = CatalogRow::validate_primary_key(
            table_schema,
            HashMap::from([(primary_key_column, key_proxima_value)]),
        )?;

        catalog_row.to_mutation_record(
            table_schema,
            RelationalMutationKind::Delete,
            RelationalRecordOptions {
                method: Some("sql_delete".to_string()),
                origin: Some("delete".to_string()),
                created_at_ns: Some(now_ns),
                updated_at_ns: Some(now_ns),
                valid_to_ns: Some(0),
                include_vector_embeddings: false,
                ..RelationalRecordOptions::default()
            },
        )
    }

    fn primary_key_string_to_proxima_value(
        column_name: &str,
        key_value: &str,
        table_schema: &CatalogTableSchema,
    ) -> Result<ProximaValue> {
        let Some(column) = table_schema
            .columns
            .iter()
            .find(|column| column.name == column_name)
        else {
            return Ok(ProximaValue::String(key_value.to_string()));
        };

        match column.data_type {
            CatalogDataType::Int8 => key_value
                .parse::<i8>()
                .map(ProximaValue::Int8)
                .map_err(|e| anyhow!("Invalid primary key value '{}': {}", key_value, e)),
            CatalogDataType::Int16 => key_value
                .parse::<i16>()
                .map(ProximaValue::Int16)
                .map_err(|e| anyhow!("Invalid primary key value '{}': {}", key_value, e)),
            CatalogDataType::Int32 => key_value
                .parse::<i32>()
                .map(ProximaValue::Int32)
                .map_err(|e| anyhow!("Invalid primary key value '{}': {}", key_value, e)),
            CatalogDataType::Int64 => key_value
                .parse::<i64>()
                .map(ProximaValue::Int64)
                .map_err(|e| anyhow!("Invalid primary key value '{}': {}", key_value, e)),
            CatalogDataType::Uuid => Ok(ProximaValue::String(key_value.to_string())),
            CatalogDataType::String => Ok(ProximaValue::String(key_value.to_string())),
            other => Err(anyhow!(
                "Primary key column '{}' with type {:?} is not supported for DELETE key extraction",
                column_name,
                other
            )),
        }
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
        let mut row_values = Self::row_values_from_existing(&existing, table_schema)?;
        let mut created_at_ns = existing
            .timestamp
            .map(|timestamp_ms| timestamp_ms.saturating_mul(1_000_000));
        let mut updated_at_ns = chrono::Utc::now()
            .timestamp_millis()
            .saturating_mul(1_000_000);

        for (column_name, value) in assignments {
            let effective_value =
                self.effective_insert_literal(column_name, value, table_schema)?;
            let proxima_value = self.literal_to_proxima_value_for_column(
                column_name,
                &effective_value,
                table_schema,
            )?;

            if column_name == "timestamp" {
                if let Some(timestamp_ms) = self.literal_to_timestamp(&effective_value)? {
                    let timestamp_ns = timestamp_ms.saturating_mul(1_000_000);
                    created_at_ns = Some(timestamp_ns);
                    updated_at_ns = timestamp_ns;
                }
            }

            row_values.insert(column_name.clone(), proxima_value);
        }

        let catalog_row =
            CatalogRow::validate(table_schema, row_values, &RelationalWriteProfile::oltp())?;
        catalog_row.to_mutation_record(
            table_schema,
            RelationalMutationKind::Update,
            RelationalRecordOptions {
                method: Some("sql_dml_update".to_string()),
                created_at_ns,
                updated_at_ns: Some(updated_at_ns),
                record_version: existing.version.map(|version| version as u64 + 1),
                ..RelationalRecordOptions::default()
            },
        )
    }

    fn row_values_from_existing(
        existing: &RichSearchResult,
        table_schema: &CatalogTableSchema,
    ) -> Result<HashMap<String, ProximaValue>> {
        let primary_key_column = Self::primary_key_column(table_schema);
        let mut row_values = HashMap::new();

        for column in &table_schema.columns {
            if primary_key_column.as_deref() == Some(column.name.as_str()) {
                row_values.insert(
                    column.name.clone(),
                    Self::primary_key_string_to_proxima_value(
                        &column.name,
                        &existing.id,
                        table_schema,
                    )?,
                );
                continue;
            }

            if matches!(column.data_type, CatalogDataType::Vector) {
                if existing.vector.is_empty() && column.nullable {
                    row_values.insert(column.name.clone(), ProximaValue::Null);
                } else {
                    row_values.insert(
                        column.name.clone(),
                        ProximaValue::DenseVector(existing.vector.clone()),
                    );
                }
                continue;
            }

            if column.name == "timestamp" {
                if let Some(timestamp_ms) = existing.timestamp {
                    row_values.insert(
                        column.name.clone(),
                        match column.data_type {
                            CatalogDataType::TimestampTz => ProximaValue::TimestampTz(
                                timestamp_ms,
                                proximadb_data_model::TimeUnit::Millisecond,
                            ),
                            _ => ProximaValue::Timestamp(
                                timestamp_ms,
                                proximadb_data_model::TimeUnit::Millisecond,
                            ),
                        },
                    );
                }
                continue;
            }

            if let Some(value) = existing.props.get(&column.name) {
                row_values.insert(column.name.clone(), value.clone());
            }
        }

        Ok(row_values)
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
            CatalogDataType::Vector => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    Ok(ProximaValue::Null)
                } else {
                    self.literal_to_vector(val).map(ProximaValue::DenseVector)
                }
            }
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
    use crate::query::table_write_executor::PlannedOnlyTableWriteExecutor;
    use crate::services::operations::batch_result::OperationMetrics;
    use crate::services::record_store::{
        TableRecordGetResponse, TableRecordScanRequest, TableRecordScanResponse,
        TableRecordWriteResult,
    };
    use crate::services::{DdlService, DdlStatement};
    use proximadb_catalog::{CatalogColumn, CatalogStorageSpecialization, CatalogWorkloadProfile};

    struct ExplainOnlyRecordStore;

    #[async_trait::async_trait]
    impl TableRecordStore for ExplainOnlyRecordStore {
        async fn write_mutations(
            &self,
            _table_schema: &CatalogTableSchema,
            mutations: Vec<TableRecordMutation>,
            _tenant_context: Option<&crate::storage::tenant::context::TenantContext>,
        ) -> Result<TableRecordWriteResult> {
            Ok(TableRecordWriteResult {
                success: true,
                record_ids: mutations
                    .into_iter()
                    .map(|mutation| mutation.record.oid)
                    .collect(),
                metrics: OperationMetrics::default(),
                errors: Vec::new(),
                error_code: None,
            })
        }

        async fn get_by_key(
            &self,
            _table_schema: &CatalogTableSchema,
            _request: TableRecordGetRequest,
            _tenant_context: Option<&crate::storage::tenant::context::TenantContext>,
        ) -> Result<TableRecordGetResponse> {
            Ok(None)
        }

        async fn scan_records(
            &self,
            _table_schema: &CatalogTableSchema,
            _request: TableRecordScanRequest,
            _tenant_context: Option<&crate::storage::tenant::context::TenantContext>,
        ) -> Result<TableRecordScanResponse> {
            Ok(Vec::new())
        }
    }

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
            .with_primary_key(vec!["record_id".to_string()])
    }

    #[test]
    fn test_resolve_select_projection_uses_catalog_columns() {
        let schema = update_test_schema();

        let projected = DmlService::resolve_select_projection(
            &schema,
            &["record_id".to_string(), "payload".to_string()],
        )
        .expect("projection should resolve");

        assert_eq!(
            projected
                .iter()
                .map(|column| column.name.as_str())
                .collect::<Vec<_>>(),
            vec!["record_id", "payload"]
        );
        assert!(DmlService::resolve_select_projection(&schema, &["missing".to_string()]).is_err());
    }

    #[test]
    fn test_project_select_rows_uses_catalog_primary_key_props_and_vectors() {
        let schema = CatalogTableSchema::new("items")
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::Int32))
            .with_column(CatalogColumn::new(2, "name", CatalogDataType::String))
            .with_column(CatalogColumn::new(3, "active", CatalogDataType::Boolean))
            .with_column(CatalogColumn::new(4, "embedding", CatalogDataType::Vector))
            .with_primary_key(vec!["id".to_string()]);
        let selected_columns = schema.columns.clone();
        let record = ProximaRecord {
            oid: "7".to_string(),
            props: proximadb_records::ProximaTree::from([
                (
                    "name".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String("alice".to_string())),
                ),
                (
                    "active".to_string(),
                    ProximaTreeNode::Value(ProximaValue::Boolean(true)),
                ),
            ]),
            embeddings: vec![EmbeddingCell {
                model_id: "default".to_string(),
                modality: "vector".to_string(),
                dim: 2,
                values: vec![0.1, 0.2],
            }],
            ..Default::default()
        };

        let rows = DmlService::project_select_rows(&[record], &schema, &selected_columns)
            .expect("row projection should succeed");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0][0], ProximaValue::Int32(7));
        assert_eq!(rows[0][1], ProximaValue::String("alice".to_string()));
        assert_eq!(rows[0][2], ProximaValue::Boolean(true));
        assert_eq!(rows[0][3], ProximaValue::DenseVector(vec![0.1, 0.2]));
    }

    #[test]
    fn test_select_route_metadata_carries_catalog_read_route() {
        let mut schema = CatalogTableSchema::new("items")
            .with_column(CatalogColumn::new(1, "id", CatalogDataType::Int32))
            .with_column(CatalogColumn::new(2, "name", CatalogDataType::String))
            .with_primary_key(vec!["id".to_string()])
            .with_workload_profile(CatalogWorkloadProfile::Oltp)
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);
        schema
            .properties
            .insert("policy_boundary".to_string(), "engine-enforced".to_string());
        let predicates = vec![RelationalSelectPredicate {
            column: schema.columns[0].clone(),
            condition: RelationalSelectPredicateCondition::Comparison {
                operator: RelationalSelectPredicateOperator::Equal,
                literal: "7".to_string(),
            },
        }];

        let route = DmlService::select_route_metadata(
            &schema,
            &schema.columns,
            &predicates,
            Some(1),
            RelationalSelectAccessPath::PrimaryKeyLookup,
        );

        assert_eq!(
            route.access_path,
            RelationalSelectAccessPath::PrimaryKeyLookup
        );
        assert_eq!(route.authority_mode, "ProximaAuthoritative");
        assert_eq!(route.workload_profile, "oltp");
        assert_eq!(route.storage_specialization, "pax_oltp");
        assert_eq!(route.policy_boundary, "engine-enforced");
        assert_eq!(route.predicate_count, 1);
        assert_eq!(route.projected_column_count, 2);
        assert_eq!(route.limit, Some(1));
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

    #[test]
    fn test_delete_tombstone_record_uses_catalog_primary_key_shape() {
        let record = DmlService::build_delete_tombstone_record("r1", &update_test_schema(), 123)
            .expect("delete tombstone should build");

        assert_eq!(record.oid, "r1");
        assert_eq!(record.local_id.as_deref(), Some("r1"));
        assert_eq!(record.variation_id.as_deref(), Some("agent_store"));
        assert_eq!(record.created_at_ns, 123);
        assert_eq!(record.updated_at_ns, 123);
        assert_eq!(record.valid_to_ns, Some(0));
        assert_eq!(record.origin.as_deref(), Some("delete"));
        assert!(record.embeddings.is_empty());
    }

    #[test]
    fn test_mutation_methods_distinguish_insert_and_upsert() {
        assert_eq!(
            DmlService::mutation_method(RelationalMutationKind::Insert),
            "sql_insert"
        );
        assert_eq!(
            DmlService::mutation_method(RelationalMutationKind::Upsert),
            "sql_upsert"
        );
    }

    #[test]
    fn test_row_dml_write_intent_routes_mutations_to_wal_lane() {
        let schema = update_test_schema();
        for operation in [
            WriteOperationKind::Insert,
            WriteOperationKind::Upsert,
            WriteOperationKind::Update,
            WriteOperationKind::Delete,
        ] {
            let (intent, decision) = DmlService::route_row_dml_write_intent(&schema, operation, 3);

            assert_eq!(intent.durability, WriteDurabilityRequirement::WalRequired);
            assert_eq!(format!("{:?}", decision.lane), "WalCurrentState");
        }
    }

    #[test]
    fn test_update_reconstructs_catalog_validated_row_shape() {
        let existing = RichSearchResult {
            id: "r1".to_string(),
            score: 1.0,
            similarity: None,
            vector: Vec::new(),
            props: HashMap::from([
                (
                    "name".to_string(),
                    ProximaValue::String("before".to_string()),
                ),
                ("notes".to_string(), ProximaValue::String("old".to_string())),
            ]),
            version: Some(7),
            timestamp: None,
            source: None,
        };
        let schema = update_test_schema();
        let mut row_values = DmlService::row_values_from_existing(&existing, &schema)
            .expect("existing row should map to catalog values");
        row_values.insert(
            "notes".to_string(),
            ProximaValue::String("updated".to_string()),
        );

        let row = CatalogRow::validate(&schema, row_values, &RelationalWriteProfile::oltp())
            .expect("updated row should validate");
        let record = row
            .to_mutation_record(
                &schema,
                RelationalMutationKind::Update,
                RelationalRecordOptions {
                    method: Some("sql_dml_update".to_string()),
                    record_version: Some(8),
                    ..RelationalRecordOptions::default()
                },
            )
            .expect("updated row should project");

        assert_eq!(record.oid, "r1");
        assert_eq!(record.record_version, 8);
        assert_eq!(record.method.as_deref(), Some("sql_dml_update"));
        assert_eq!(
            proximadb_records::tree_get(&record.props, "notes"),
            Some(&ProximaValue::String("updated".to_string()))
        );
    }

    #[tokio::test]
    async fn test_explain_table_write_returns_route_explanation() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");

        let ddl = DdlService::new(manager.clone());
        ddl.execute(DdlStatement::CreateNamespace {
            namespace: vec!["default".to_string()],
            if_not_exists: true,
            properties: HashMap::new(),
        })
        .await
        .expect("create namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        for ddl_sql in [
            "CREATE TABLE staging (id TEXT NOT NULL, payload JSONB);",
            "CREATE TABLE facts (id TEXT NOT NULL, payload JSONB)
             WITH (
                workload = 'olap',
                layout = 'columnar',
                compute_route = 'datafusion-local',
                freshness_sla = '5s'
             );",
        ] {
            let statement = parser
                .parse_ddl(ddl_sql)
                .expect("parse ddl")
                .expect("ddl statement");
            ddl.execute(statement).await.expect("execute ddl");
        }

        for (table_name, stats) in [
            (
                "staging",
                CatalogTableStatistics {
                    row_count: 1_000,
                    size_bytes: 512_000,
                    file_count: 1,
                    ..Default::default()
                },
            ),
            (
                "facts",
                CatalogTableStatistics {
                    row_count: 10_000,
                    size_bytes: 4_000_000,
                    file_count: 4,
                    ..Default::default()
                },
            ),
        ] {
            let (catalog, table_id) = manager
                .resolve_table(table_name)
                .await
                .expect("resolve table for stats");
            catalog
                .update_statistics(&table_id, stats)
                .await
                .expect("update stats");
        }

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(ExplainOnlyRecordStore),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        let statement = parser
            .parse_dml("INSERT INTO facts SELECT * FROM staging;")
            .expect("parse dml")
            .expect("dml statement");

        let explanation = dml
            .explain_table_write(statement)
            .await
            .expect("explain route");

        assert_eq!(explanation.target_table, "facts");
        assert_eq!(explanation.selected_backend, "DataFusionLocal");
        assert_eq!(explanation.route_metadata.workload_profile, "olap");
        assert_eq!(
            explanation.route_metadata.storage_specialization,
            "columnar_analytics"
        );
        assert_eq!(
            explanation
                .route_metadata
                .preferred_compute_route
                .as_deref(),
            Some("datafusion-local")
        );
        assert_eq!(
            explanation.route_metadata.freshness_sla.as_deref(),
            Some("5s")
        );
        assert_eq!(explanation.data_movement.source_rows, Some(1_000));
        assert_eq!(explanation.data_movement.source_bytes, Some(512_000));
        assert_eq!(
            explanation.data_movement.target_bytes_before_write,
            Some(4_000_000)
        );
        assert!(
            explanation
                .candidate_paths
                .iter()
                .any(|path| path.backend == "DataFusionLocal")
        );
    }

    /// End-to-end smoke test: `DmlService` with a `DirectWalTableRecordStore` performs a
    /// VALUES INSERT and a primary-key SELECT through the canonical WAL + memtable path,
    /// then replays the WAL into a fresh memtable to verify durability.
    #[tokio::test]
    async fn direct_record_storage_insert_select_and_wal_replay() {
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};
        use crate::services::record_store::DirectWalTableRecordStore;

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("dml-smoke.wal");

        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");

        let ddl = DdlService::new(manager.clone());
        ddl.execute(DdlStatement::CreateNamespace {
            namespace: vec!["default".to_string()],
            if_not_exists: true,
            properties: HashMap::new(),
        })
        .await
        .expect("create namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let create_sql = "CREATE TABLE users (id TEXT NOT NULL, email TEXT NOT NULL, age INT, PRIMARY KEY (id));";
        let ddl_stmt = parser
            .parse_ddl(create_sql)
            .expect("parse create table")
            .expect("ddl statement");
        ddl.execute(ddl_stmt).await.expect("create table");

        // Wire DmlService over the canonical direct WAL writer.
        let wal_appender = Arc::new(
            FramedTableWalAppender::open(&wal_path)
                .await
                .expect("open WAL"),
        );
        let record_storage = Arc::new(MemtableRecordStorage::new());
        let record_store = Arc::new(DirectWalTableRecordStore::new(
            record_storage.clone(),
            wal_appender.clone(),
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        // INSERT via SQL VALUES DML.
        let insert_sql =
            "INSERT INTO users (id, email, age) VALUES ('u1', 'alice@example.com', 30);";
        let stmt = parser
            .parse_dml(insert_sql)
            .expect("parse insert")
            .expect("dml statement");
        let result = dml.execute(stmt).await.expect("execute insert");
        assert!(result.success, "INSERT must succeed");
        assert_eq!(result.rows_affected, 1);

        // SELECT via DmlService's relational scan/projection path — verifies current-state is
        // visible through the canonical record store.
        let sel = dml
            .select_table_records_with_projection(
                "users",
                &["id".to_string(), "email".to_string(), "age".to_string()],
                None,
                &[],
            )
            .await
            .expect("select users");
        assert_eq!(sel.rows.len(), 1, "SELECT must return the inserted row");
        assert_eq!(
            sel.rows[0][0],
            ProximaValue::String("u1".to_string()),
            "id column must match"
        );
        assert_eq!(
            sel.rows[0][1],
            ProximaValue::String("alice@example.com".to_string()),
            "email column must match"
        );

        // Replay WAL into a fresh memtable — verifies Layer 0 durability.
        let replay_storage = Arc::new(MemtableRecordStorage::new());
        let replay_wal = FramedTableWalAppender::open(&wal_path)
            .await
            .expect("reopen WAL for replay");
        let entries = replay_wal
            .read_entries()
            .await
            .expect("read WAL entries");
        assert!(!entries.is_empty(), "WAL must contain at least one entry");
        let summary = replay_storage
            .replay_wal_entries(entries)
            .await
            .expect("replay WAL");
        assert_eq!(
            summary.upserts_replayed, 1,
            "WAL replay must recover the INSERT as an upsert"
        );
        assert_eq!(
            replay_storage.len(),
            1,
            "replayed memtable must hold the record"
        );
    }

    /// SQL UPDATE and DELETE through `DirectWalTableRecordStore` — T9 conformance.
    ///
    /// Verifies that UPDATE rewrites the current visible record and DELETE leaves
    /// the row invisible to subsequent scans, both through the canonical WAL path.
    #[tokio::test]
    async fn direct_record_storage_update_and_delete_conformance() {
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};
        use crate::services::record_store::DirectWalTableRecordStore;

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("dml-ud.wal");

        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");

        let ddl = DdlService::new(manager.clone());
        ddl.execute(DdlStatement::CreateNamespace {
            namespace: vec!["default".to_string()],
            if_not_exists: true,
            properties: HashMap::new(),
        })
        .await
        .expect("create namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let create_sql = "CREATE TABLE items (id TEXT NOT NULL, label TEXT, PRIMARY KEY (id));";
        let ddl_stmt = parser
            .parse_ddl(create_sql)
            .expect("parse ddl")
            .expect("ddl stmt");
        ddl.execute(ddl_stmt).await.expect("create table");

        let wal_appender = Arc::new(
            FramedTableWalAppender::open(&wal_path)
                .await
                .expect("open WAL"),
        );
        let record_storage = Arc::new(MemtableRecordStorage::new());
        let record_store = Arc::new(DirectWalTableRecordStore::new(
            record_storage.clone(),
            wal_appender,
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        // INSERT two rows.
        for (id, label) in [("i1", "alpha"), ("i2", "beta")] {
            let sql = format!(
                "INSERT INTO items (id, label) VALUES ('{}', '{}');",
                id, label
            );
            let stmt = parser
                .parse_dml(&sql)
                .expect("parse insert")
                .expect("dml stmt");
            let r = dml.execute(stmt).await.expect("insert");
            assert!(r.success, "INSERT {id} must succeed");
        }

        // UPDATE i1's label.
        let update_sql = "UPDATE items SET label = 'alpha-updated' WHERE id = 'i1';";
        let update_stmt = parser
            .parse_dml(update_sql)
            .expect("parse update")
            .expect("dml stmt");
        let update_r = dml.execute(update_stmt).await.expect("update");
        assert!(update_r.success, "UPDATE must succeed");

        // Verify the updated label is visible via SELECT projection.
        let after_update = dml
            .select_table_records_with_projection(
                "items",
                &["id".to_string(), "label".to_string()],
                None,
                &[RelationalSelectPredicateInput {
                    column_name: "id".to_string(),
                    condition: RelationalSelectPredicateCondition::Comparison {
                        operator: RelationalSelectPredicateOperator::Equal,
                        literal: "i1".to_string(),
                    },
                }],
            )
            .await
            .expect("select after update");
        assert_eq!(after_update.rows.len(), 1, "SELECT must find i1 after update");
        assert_eq!(
            after_update.rows[0][1],
            ProximaValue::String("alpha-updated".to_string()),
            "updated label must be visible"
        );

        // DELETE i2.
        let delete_sql = "DELETE FROM items WHERE id = 'i2';";
        let delete_stmt = parser
            .parse_dml(delete_sql)
            .expect("parse delete")
            .expect("dml stmt");
        let delete_r = dml.execute(delete_stmt).await.expect("delete");
        assert!(delete_r.success, "DELETE must succeed");

        // Verify i2 is no longer returned by a full scan.
        let after_delete = dml
            .select_table_records_with_projection(
                "items",
                &["id".to_string()],
                None,
                &[],
            )
            .await
            .expect("select after delete");
        let ids: Vec<&ProximaValue> = after_delete.rows.iter().map(|r| &r[0]).collect();
        assert!(
            !ids.contains(&&ProximaValue::String("i2".to_string())),
            "deleted row must not appear in scan"
        );
        assert_eq!(after_delete.rows.len(), 1, "only i1 must remain");
    }
}
