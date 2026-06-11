//! DML (Data Manipulation Language) Service
//!
//! Provides SQL DML operations that integrate with the catalog and storage system:
//! - INSERT INTO ... VALUES (...)
//! - UPDATE ... SET ... WHERE ...
//! - DELETE FROM ... WHERE ...
//! - UPSERT / INSERT ... ON CONFLICT ...

use std::collections::HashMap;
use std::sync::Arc;

use anyhow::{Context, Result, anyhow};
use proximadb_catalog::{
    CatalogColumn, CatalogStorageLayout, CatalogTableSchema, CatalogTableStatistics,
    relational::{
        CatalogRow, RelationalMutationKind, RelationalRecordOptions, RelationalWriteProfile,
    },
};
use proximadb_data_model::ProximaType;
use proximadb_data_model::ProximaValue;
use proximadb_records::{EmbeddingCell, ProximaRecord, ProximaTreeNode, RecordStorage};
use tracing::{debug, info, warn};

use crate::catalog::CatalogManager;
use crate::query::table_write_executor::{
    DataFusionTableWriteExecutor, NativeTableWriteExecutor, PlannedOnlyTableWriteExecutor,
    TableRecordStoreSourceReader, TableWriteExecutionRequest, TableWriteExecutionStatus,
    TableWriteExecutor,
};
use crate::query::table_write_plan::{
    ConflictPolicy, CopyIntoPlan, DistributionMode, DmlWritePlanRequest, DmlWritePlanner,
    LogicalTableRef, ReadSource, RoutedExecutionPlan, TableWriteRouteExplanation,
    WriteIntentOverrides, WriteMode,
};
use crate::storage::tenant::context::TenantContext;
use crate::services::operations::VectorOps;
use crate::services::operations::vectors::RichSearchResult;
use crate::services::record_store::{
    CatalogRoutingTableRecordStore, DirectWalTableRecordStore, ObjectStoreIcebergRecordStore,
    ObjectStoreVectorRecordStore, TableRecordGetRequest, TableRecordMutation,
    TableRecordMutationKind, TableRecordScanRequest, TableRecordStore, TableWalAppender,
    VectorOpsTableRecordStore, proxima_value_to_unique_text, record_unique_tuple,
};
use crate::services::{
    WriteDurabilityRequirement, WriteIntent, WriteLaneDecision, WriteLaneRouter, WriteOperationKind,
};
use proximadb_storage_common::object_store_bridge::ObjectStoreBridge;

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

/// Resolved boolean predicate tree for UPDATE/DELETE `WHERE` evaluation. Leaves
/// are the same catalog-resolved [`RelationalSelectPredicate`] used by SELECT;
/// the And/Or/Not nodes let UPDATE/DELETE honor full boolean `WHERE` clauses
/// (OR, nested groups, NOT BETWEEN) while reusing the exact same catalog-aware
/// leaf comparison (`compare_catalog_value`, PK-by-oid resolution).
#[derive(Debug, Clone)]
enum RelationalPredicateTree {
    Leaf(RelationalSelectPredicate),
    And(Vec<RelationalPredicateTree>),
    Or(Vec<RelationalPredicateTree>),
    Not(Box<RelationalPredicateTree>),
}

impl RelationalPredicateTree {
    /// Number of leaf (column) predicates in the tree — used as the
    /// `predicate_count` route-metadata signal for tree-driven SELECTs (the
    /// tree analogue of the flat path's `predicates.len()`).
    fn leaf_count(&self) -> usize {
        match self {
            Self::Leaf(_) => 1,
            Self::And(children) | Self::Or(children) => children.iter().map(Self::leaf_count).sum(),
            Self::Not(child) => child.leaf_count(),
        }
    }
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

    /// Create a DML service with the native direct canonical writer enabled.
    ///
    /// Mutable Proxima-owned relational/OLTP/HTAP tables route through
    /// `DirectWalTableRecordStore` into canonical WAL plus the `RecordStorage`
    /// row/delta spine. OLAP/open-format publication is a projection of that
    /// state unless xCatalog declares an explicit external authority. Legacy
    /// vector/LSM-specialized tables still route through the VectorOps
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
            // Temporary wiring during migration: canonical acts as the native
            // row/delta path, legacy acts as the vector-specialized path.
            CatalogRoutingTableRecordStore::new(
                canonical_store.clone(),
                legacy_store.clone(),
                legacy_store,
            ),
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

    /// Create a DML service with the decoupled object-store routes enabled.
    ///
    /// Relational/analytics table writes route to the Parquet/Iceberg object
    /// store path, vector-specialized writes route to the PAX/vector object
    /// store path, and legacy vector/LSM tables keep the VectorOps adapter.
    pub fn with_object_store_bridge(
        catalog_manager: Arc<CatalogManager>,
        vector_ops: Arc<VectorOps>,
        bridge: Arc<dyn ObjectStoreBridge>,
    ) -> Self {
        let iceberg_store = Arc::new(ObjectStoreIcebergRecordStore::new(bridge.clone()));
        let vector_store = Arc::new(ObjectStoreVectorRecordStore::new(bridge.clone()));
        let legacy_store = Arc::new(VectorOpsTableRecordStore::new(vector_ops));
        let routed_store: Arc<dyn TableRecordStore> = Arc::new(
            CatalogRoutingTableRecordStore::new(iceberg_store, vector_store, legacy_store),
        );
        let source_reader = Arc::new(TableRecordStoreSourceReader::new(routed_store.clone()));
        let table_write_executor = Arc::new(
            DataFusionTableWriteExecutor::new(source_reader, routed_store.clone())
                .with_object_store_bridge(bridge),
        );

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
                self.plan_table_write(&plan, &columns, None).await?
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
            predicates.len(),
            limit,
            access_path,
        );

        Ok(RelationalSelectResult {
            selected_columns,
            route_metadata,
            rows,
        })
    }

    /// Select current visible records for a catalog table using the same faithful
    /// boolean [`WhereClause`] IR that UPDATE/DELETE use — so OR / mixed-AND-OR /
    /// grouped SELECT predicates push into the scan instead of degrading to a full
    /// table scan. `where_clause = None` means no `WHERE` (scan all, capped by limit).
    ///
    /// This converges the pgwire SELECT WHERE path onto [`RelationalPredicateTree`]
    /// (built by [`Self::where_clause_to_predicate_tree`]) and reuses the exact
    /// OR-safe PK fast-path ([`Self::extract_pk_candidate_ids`]) + tree evaluator
    /// ([`Self::eval_predicate_tree`]) that drive UPDATE/DELETE. The flat
    /// [`Self::select_table_records_with_projection`] is retained unchanged for
    /// callers that still pass a flat predicate Vec.
    pub async fn select_table_records_with_projection_where(
        &self,
        table_name: &str,
        projection_column_names: &[String],
        limit: Option<usize>,
        where_clause: Option<&WhereClause>,
    ) -> Result<RelationalSelectResult> {
        let (table_schema, table_id_name) = self.resolve_select_table(table_name).await?;
        let selected_columns =
            Self::resolve_select_projection(&table_schema, projection_column_names)?;

        let (access_path, records, predicate_count) = match where_clause {
            Some(where_clause) => {
                let tree = self.where_clause_to_predicate_tree(where_clause, &table_schema)?;
                let predicate_count = tree.leaf_count();
                let (access_path, records) = self
                    .select_table_records_with_tree(
                        &table_schema,
                        &table_id_name,
                        limit,
                        where_clause,
                        &tree,
                    )
                    .await?;
                (access_path, records, predicate_count)
            }
            None => {
                // No WHERE: scan all rows up to the limit (no predicate pushed).
                let records = self
                    .record_store
                    .scan_records_filtered(
                        &table_schema,
                        TableRecordScanRequest {
                            table_id: table_id_name.clone(),
                            limit,
                            include_vector: true,
                            include_props: true,
                        },
                        None,
                        None,
                    )
                    .await?;
                (RelationalSelectAccessPath::TableScan, records, 0)
            }
        };

        let rows = Self::project_select_rows(&records, &table_schema, &selected_columns)?;
        let route_metadata = Self::select_route_metadata(
            &table_schema,
            &selected_columns,
            predicate_count,
            limit,
            access_path,
        );

        Ok(RelationalSelectResult {
            selected_columns,
            route_metadata,
            rows,
        })
    }

    /// Resolve a catalog table's schema for the relational pipeline (PATH B).
    /// Used by the pipeline's schema-only prefetch (the sync `CatalogLookup`
    /// can't await xCatalog), so the actual rows can be fetched lazily per scan.
    pub async fn resolve_relational_schema(&self, table_name: &str) -> Result<CatalogTableSchema> {
        let (table_schema, _table_id_name) = self.resolve_select_table(table_name).await?;
        Ok(table_schema)
    }

    /// Scan a catalog table for the relational pipeline (PATH B), pushing the
    /// caller's row predicate + limit into the record-store scan and projecting
    /// matching records into `output_columns` order (or all columns when `None`).
    ///
    /// The predicate is `Expr`-agnostic: callers pass a closure over a FULL row
    /// (all columns in `schema.columns` order — matching how the relational
    /// `Expr` binds its column ordinals), so the relational reader can supply
    /// `|row| expr.eval(row) == true` without coupling DmlService to the algebra
    /// crate. `MemtableRecordStorage::scan_records_filtered` iterate-filters and
    /// early-stops at `limit`, so only matching rows are cloned/materialized.
    pub async fn scan_table_relational(
        &self,
        table_name: &str,
        output_columns: Option<&[String]>,
        full_row_predicate: Option<&(dyn Fn(&[ProximaValue]) -> bool + Send + Sync)>,
        limit: Option<usize>,
    ) -> Result<(CatalogTableSchema, Vec<Vec<ProximaValue>>)> {
        let (table_schema, table_id_name) = self.resolve_select_table(table_name).await?;
        // Full column set — predicates are evaluated against a complete row.
        let all_columns: Vec<String> = table_schema
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect();
        let full_selected = Self::resolve_select_projection(&table_schema, &all_columns)?;
        // Output column set the caller wants emitted (defaults to all columns).
        let output_selected = match output_columns {
            Some(cols) => Self::resolve_select_projection(&table_schema, cols)?,
            None => full_selected.clone(),
        };

        // Push the predicate INTO the store scan: project each record to a full
        // row and apply the caller's full-row predicate. A projection error
        // (should not happen for a valid record) excludes the row.
        let record_pred = |record: &ProximaRecord| -> bool {
            match Self::project_one_record(record, &table_schema, &full_selected) {
                Ok(full_row) => full_row_predicate.map_or(true, |p| p(&full_row)),
                Err(_) => false,
            }
        };
        let predicate: Option<&proximadb_records::RecordScanPredicate<'_>> = Some(&record_pred);

        let records = self
            .record_store
            .scan_records_filtered(
                &table_schema,
                TableRecordScanRequest {
                    table_id: table_id_name.clone(),
                    limit,
                    include_vector: true,
                    include_props: true,
                },
                predicate,
                None,
            )
            .await?;

        let rows = Self::project_select_rows(&records, &table_schema, &output_selected)?;
        Ok((table_schema, rows))
    }

    /// Materialize a relational table's current rows as a Parquet snapshot on object
    /// storage and flip its catalog storage layout to `Parquet` /
    /// `ProjectionPublication`, so the OLAP router's `catalog_table_is_parquet_backed`
    /// check passes and SELECTs over the table route to DataFusion.
    ///
    /// This is the explicit-publish half of the dual-path design (course-correction
    /// §6 P3): OLTP rows stay authoritative in RecordStorage; this publishes a
    /// read-optimized Parquet projection of the current snapshot. It is triggered
    /// explicitly (e.g. `ALTER TABLE … MATERIALIZE`), not on every write.
    ///
    /// `bridge` is the object store to write into; `warehouse_root_url` is the URL the
    /// OLAP reader reopens that same physical store from, so the published
    /// `location = {warehouse_root_url}/{tenant-isolated prefix}` resolves back to the
    /// data the reader lists as `{location}/data/*.parquet`. Returns the published
    /// `location`.
    ///
    /// MVP scope: a single Parquet object per materialization (full overwrite
    /// snapshot), schema inferred from the rows. Incremental/atomic manifest
    /// publication (`IcebergObjectStoreBridge::publish_snapshot`) and the
    /// catalog-authoritative write schema are follow-ups.
    pub async fn materialize_table_to_parquet(
        &self,
        bridge: &dyn ObjectStoreBridge,
        warehouse_root_url: &str,
        table_name: &str,
        tenant_context: Option<&TenantContext>,
    ) -> Result<String> {
        // 1. Snapshot the table's current rows (all columns, no predicate/limit).
        let (schema, rows) = self
            .scan_table_relational(table_name, None, None, None)
            .await?;

        // 2. Column-order ProximaValue rows → ProximaRecord envelopes (props keyed by
        //    column name; relational tables carry no vectors). NULLs are omitted —
        //    the schema-driven Arrow mapping null-fills any absent column.
        let col_names: Vec<String> = schema.columns.iter().map(|c| c.name.clone()).collect();
        let records: Vec<ProximaRecord> = rows
            .into_iter()
            .enumerate()
            .map(|(i, row)| {
                let mut rec = ProximaRecord {
                    oid: i.to_string(),
                    ..Default::default()
                };
                for (name, value) in col_names.iter().zip(row.into_iter()) {
                    if !matches!(value, ProximaValue::Null) {
                        rec.props
                            .insert(name.clone(), ProximaTreeNode::Value(value));
                    }
                }
                rec
            })
            .collect();

        // 3. Tenant-isolated object prefix (DrPathBuilder mandate: data/{tenant}/{ns}/{table}).
        let tenant_id = tenant_context
            .map(|tc| tc.tenant_id.as_str())
            .unwrap_or("default_tenant");
        let prefix = format!("data/{tenant_id}/default/{}", schema.name);

        // 4. Write the snapshot under `{prefix}/data/` — exactly where the OLAP reader
        //    lists `{location}/data/*.parquet`.
        let data_object =
            object_store::path::Path::from(format!("{prefix}/data/part-0.parquet"));
        bridge
            .write_records_to_parquet(&data_object, &records, Some(tenant_id))
            .await?;

        // 5. Flip the catalog layout to a published Parquet projection at the location.
        let location = format!("{}/{prefix}", warehouse_root_url.trim_end_matches('/'));
        let (catalog, table_id) = self.catalog_manager.resolve_table(table_name).await?;
        let layout = CatalogStorageLayout {
            name: "parquet-snapshot".to_string(),
            authority: proximadb_catalog::CatalogAuthorityMode::ProjectionPublication,
            physical_format: proximadb_catalog::CatalogPhysicalFormat::Parquet,
            location: Some(location.clone()),
            ..Default::default()
        };
        catalog.set_storage_layouts(&table_id, vec![layout]).await?;

        Ok(location)
    }

    /// Point-lookup a single relational row by primary key, projected into the
    /// FULL `schema.columns` order (the executor re-applies any projection).
    ///
    /// Extends the canonical [`DmlService`]; the OLTP point-read fast path for the
    /// relational pipeline (PATH B) — the storage-side backing for
    /// `ScanAccess::PkLookup`. Single-column PK only (`get_by_key` keys on
    /// `record.oid`, matching the SELECT PK fast-path's `primary_key.first()`).
    /// ADR-018 Phase 2 (pgwire SQL parity); TD-076. Reuses the exact primitives the
    /// SELECT PK fast-path uses (`get_by_key` + `rich_result_to_record`).
    pub async fn point_lookup_relational(
        &self,
        table_name: &str,
        key: &str,
    ) -> Result<Option<Vec<ProximaValue>>> {
        let (table_schema, table_id_name) = self.resolve_select_table(table_name).await?;
        let all_columns: Vec<String> = table_schema
            .columns
            .iter()
            .map(|c| c.name.clone())
            .collect();
        let full_selected = Self::resolve_select_projection(&table_schema, &all_columns)?;
        let record = self
            .record_store
            .get_by_key(
                &table_schema,
                TableRecordGetRequest {
                    table_id: table_id_name,
                    key: key.to_string(),
                    include_vector: true,
                    include_props: true,
                },
                None,
            )
            .await?;
        match record {
            Some(rich) => {
                let record = Self::rich_result_to_record(rich);
                let row = Self::project_one_record(&record, &table_schema, &full_selected)?;
                Ok(Some(row))
            }
            None => Ok(None),
        }
    }

    /// Records-returning, limit-honoring twin of [`Self::resolve_matching_ids`] for
    /// the SELECT path: PK fast-path (OR-safe via [`Self::extract_pk_candidate_ids`],
    /// re-checked against the full tree) else a predicate scan with the limit pushed in.
    async fn select_table_records_with_tree(
        &self,
        table_schema: &CatalogTableSchema,
        table_id_name: &str,
        limit: Option<usize>,
        where_clause: &WhereClause,
        tree: &RelationalPredicateTree,
    ) -> Result<(RelationalSelectAccessPath, Vec<ProximaRecord>)> {
        let primary_key = table_schema.primary_key.first().map(String::as_str);

        // PK fast-path: only fires under a top-level conjunction (extract_pk_candidate_ids
        // returns empty under top-level OR, so `id = 5 OR status = 'x'` correctly scans).
        // Each candidate is re-checked against the FULL tree before counting as a match.
        let pk_candidates = self.extract_pk_candidate_ids(where_clause, table_schema)?;
        if !pk_candidates.is_empty() {
            let cap = limit.unwrap_or(usize::MAX);
            let mut records = Vec::new();
            for candidate in pk_candidates {
                if records.len() >= cap {
                    break;
                }
                let Some(rich) = self
                    .record_store
                    .get_by_key(
                        table_schema,
                        TableRecordGetRequest {
                            table_id: table_id_name.to_string(),
                            key: candidate,
                            include_vector: true,
                            include_props: true,
                        },
                        None,
                    )
                    .await?
                else {
                    continue;
                };
                let record = Self::rich_result_to_record(rich);
                if Self::eval_predicate_tree(&record, tree, primary_key) {
                    records.push(record);
                }
            }
            return Ok((RelationalSelectAccessPath::PrimaryKeyLookup, records));
        }

        // No usable PK predicate: push the full tree into the store scan + limit.
        let pred = |record: &ProximaRecord| Self::eval_predicate_tree(record, tree, primary_key);
        let predicate: Option<&proximadb_records::RecordScanPredicate<'_>> = Some(&pred);
        let records = self
            .record_store
            .scan_records_filtered(
                table_schema,
                TableRecordScanRequest {
                    table_id: table_id_name.to_string(),
                    limit,
                    include_vector: true,
                    include_props: true,
                },
                predicate,
                None,
            )
            .await?;
        Ok((RelationalSelectAccessPath::TableScan, records))
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
                        Self::record_matches_select_predicates(record, predicates, primary_key)
                    })
                    .into_iter()
                    .take(limit.unwrap_or(usize::MAX))
                    .collect(),
            ));
        }

        // Push the resolved `WHERE` predicate + limit INTO the scan so the store
        // filters during iteration and stops at the limit — instead of cloning
        // the whole table (embeddings included) and filtering in memory here.
        let primary_key = table_schema.primary_key.first().map(String::as_str);
        let pred = |record: &ProximaRecord| {
            Self::record_matches_select_predicates(record, predicates, primary_key)
        };
        let predicate: Option<&proximadb_records::RecordScanPredicate<'_>> =
            if predicates.is_empty() {
                None
            } else {
                Some(&pred)
            };
        let records = self
            .record_store
            .scan_records_filtered(
                table_schema,
                TableRecordScanRequest {
                    table_id: table_id_name.to_string(),
                    limit,
                    include_vector: true,
                    include_props: true,
                },
                predicate,
                None,
            )
            .await?;

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

    /// Plan the write and then execute it, returning the route explanation enriched with actual
    /// wall-clock time and rows written. Used for `EXPLAIN ANALYZE <DML>`.
    pub async fn explain_analyze_table_write(
        &self,
        statement: DmlStatement,
    ) -> Result<TableWriteRouteExplanation> {
        let mut explanation = self.explain_table_write(statement.clone()).await?;
        let t0 = std::time::Instant::now();
        let result = self.execute(statement).await?;
        let elapsed_us = t0.elapsed().as_micros() as u64;
        explanation.execution_elapsed_us = Some(elapsed_us);
        explanation.execution_rows_written = Some(result.rows_affected);
        Ok(explanation)
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
            // VALUES-based DML: synthesize a CopyIntoPlan for route planning.
            // ReadSource::QuerySql acts as a sentinel for inline-values writes;
            // the row count hint drives bulk-vs-WAL lane selection.
            DmlStatement::Insert {
                table_name,
                columns,
                values,
            } => {
                let row_count = values.len() as u64;
                let plan = CopyIntoPlan::insert_select(LogicalTableRef::new(&table_name), "VALUES");
                let mut overrides = write_intent_overrides.cloned().unwrap_or_default();
                overrides.row_count_hint = Some(row_count);
                let plan = CopyIntoPlan {
                    write_mode: WriteMode::InsertOnly,
                    ..plan
                };
                let routed = self
                    .route_table_write_plan_with_overrides(&plan, &columns, Some(&overrides))
                    .await?;
                Ok(routed.route_explanation())
            }
            DmlStatement::Upsert {
                table_name,
                columns,
                values,
                ..
            } => {
                let row_count = values.len() as u64;
                let mut overrides = write_intent_overrides.cloned().unwrap_or_default();
                overrides.row_count_hint = Some(row_count);
                let plan = CopyIntoPlan {
                    source: ReadSource::QuerySql("VALUES".to_string()),
                    target: LogicalTableRef::new(&table_name),
                    write_mode: WriteMode::Upsert,
                    conflict_policy: ConflictPolicy::Upsert,
                    distribution: DistributionMode::Auto,
                };
                let routed = self
                    .route_table_write_plan_with_overrides(&plan, &columns, Some(&overrides))
                    .await?;
                Ok(routed.route_explanation())
            }
            DmlStatement::Update { table_name, .. } => {
                let plan = CopyIntoPlan {
                    source: ReadSource::QuerySql("UPDATE".to_string()),
                    target: LogicalTableRef::new(&table_name),
                    write_mode: WriteMode::Upsert,
                    conflict_policy: ConflictPolicy::Upsert,
                    distribution: DistributionMode::Auto,
                };
                let routed = self
                    .route_table_write_plan_with_overrides(&plan, &[], write_intent_overrides)
                    .await?;
                Ok(routed.route_explanation())
            }
            DmlStatement::Delete { table_name, .. } => {
                let plan = CopyIntoPlan {
                    source: ReadSource::QuerySql("DELETE".to_string()),
                    target: LogicalTableRef::new(&table_name),
                    write_mode: WriteMode::Append,
                    conflict_policy: ConflictPolicy::Error,
                    distribution: DistributionMode::Auto,
                };
                let routed = self
                    .route_table_write_plan_with_overrides(&plan, &[], write_intent_overrides)
                    .await?;
                Ok(routed.route_explanation())
            }
        }
    }

    /// Resolve catalog metadata and route table-to-table writes before execution.
    async fn plan_table_write(
        &self,
        plan: &CopyIntoPlan,
        target_columns: &[String],
        tenant_context: Option<&TenantContext>,
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
                tenant_context,
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
        predicate_count: usize,
        limit: Option<usize>,
        access_path: RelationalSelectAccessPath,
    ) -> RelationalSelectRouteMetadata {
        RelationalSelectRouteMetadata {
            access_path,
            authority_mode: Self::select_authority_mode(table_schema),
            workload_profile: table_schema.workload_profile.as_str().to_string(),
            storage_specialization: table_schema.storage_specialization.as_str().to_string(),
            policy_boundary: Self::select_policy_boundary(table_schema),
            predicate_count,
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
                values: proximadb_records::EmbeddingValues::Fp32(result.vector),
                ..Default::default()
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
        records
            .iter()
            .map(|record| Self::project_one_record(record, table_schema, selected_columns))
            .collect()
    }

    /// Project a single record into `selected_columns` order. Extracted so the
    /// relational-pipeline scan can project per-record inside a scan-filter
    /// closure (predicate evaluation against a full row) without batching.
    fn project_one_record(
        record: &ProximaRecord,
        table_schema: &CatalogTableSchema,
        selected_columns: &[CatalogColumn],
    ) -> Result<Vec<ProximaValue>> {
        let primary_key = table_schema.primary_key.first().map(String::as_str);
        selected_columns
            .iter()
            .map(|column| {
                Self::record_column_value_for_select(record, column, primary_key, table_schema)
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
            ProximaType::DenseVector { .. }
                | ProximaType::SparseVector { .. }
                | ProximaType::BinaryVector { .. }
        ) && let Some(embedding) = record.embeddings.first()
        {
            return Ok(ProximaValue::DenseVector(embedding.values.to_fp32_owned()));
        }
        Ok(ProximaValue::Null)
    }

    /// Evaluate simple catalog-shaped predicates against a canonical record.
    pub fn record_matches_select_predicates(
        record: &ProximaRecord,
        predicates: &[RelationalSelectPredicate],
        primary_key: Option<&str>,
    ) -> bool {
        predicates
            .iter()
            .all(|predicate| Self::eval_predicate_leaf(record, predicate, primary_key))
    }

    /// Evaluate a single catalog-resolved leaf predicate against a record
    /// (catalog-type-aware comparison, PK resolved via `record.oid`). Shared by
    /// the flat SELECT matcher and the UPDATE/DELETE predicate tree.
    fn eval_predicate_leaf(
        record: &ProximaRecord,
        predicate: &RelationalSelectPredicate,
        primary_key: Option<&str>,
    ) -> bool {
        let value = Self::record_column_value_for_predicate(record, &predicate.column, primary_key);
        match &predicate.condition {
            RelationalSelectPredicateCondition::Comparison { operator, literal } => {
                Self::compare_catalog_value(&value, literal, *operator, &predicate.column.data_type)
            }
            RelationalSelectPredicateCondition::In { literals, negated } => {
                let matches = literals.iter().any(|literal| {
                    Self::compare_catalog_value(
                        &value,
                        literal,
                        RelationalSelectPredicateOperator::Equal,
                        &predicate.column.data_type,
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
    }

    /// Recursively evaluate a boolean predicate tree against a record.
    fn eval_predicate_tree(
        record: &ProximaRecord,
        tree: &RelationalPredicateTree,
        primary_key: Option<&str>,
    ) -> bool {
        match tree {
            RelationalPredicateTree::Leaf(predicate) => {
                Self::eval_predicate_leaf(record, predicate, primary_key)
            }
            RelationalPredicateTree::And(children) => children
                .iter()
                .all(|child| Self::eval_predicate_tree(record, child, primary_key)),
            RelationalPredicateTree::Or(children) => children
                .iter()
                .any(|child| Self::eval_predicate_tree(record, child, primary_key)),
            RelationalPredicateTree::Not(child) => {
                !Self::eval_predicate_tree(record, child, primary_key)
            }
        }
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
        // Vector-typed columns are not meaningfully comparable via SQL scalar
        // predicates; emitting a CSV stringification per row (potentially KB-sized)
        // is wasted work and the comparison can never succeed semantically. Return
        // empty so IsNull behaves correctly and all ordered/equality predicates
        // evaluate to false.
        String::new()
    }

    fn proxima_value_to_predicate_text(value: &ProximaValue) -> String {
        // Single source of truth shared with the record store (TD-110 Slice C),
        // so the value rendered for a UNIQUE/PK index/probe matches the value
        // rendered for predicate evaluation.
        proxima_value_to_unique_text(value)
    }

    fn compare_catalog_value(
        value: &str,
        literal: &str,
        operator: RelationalSelectPredicateOperator,
        data_type: &ProximaType,
    ) -> bool {
        match data_type {
            ProximaType::Boolean => {
                let left = Self::normalize_bool_literal(value);
                let right = Self::normalize_bool_literal(literal);
                Self::compare_ordered_values(left, right, operator)
            }
            ProximaType::Int8
            | ProximaType::Int16
            | ProximaType::Int32
            | ProximaType::Int64
            | ProximaType::Float32
            | ProximaType::Float64
            | ProximaType::Decimal { .. } => {
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
        // Fast paths for common patterns. Only applied for pure-ASCII patterns
        // where byte-indexed slicing is equivalent to char-indexed slicing; this
        // preserves the char-based semantics of the DP path below for multi-byte
        // patterns. `_` is not handled here because it requires per-position
        // wildcard checking.
        if pattern.is_ascii() && !pattern.contains('_') {
            let bytes = pattern.as_bytes();
            let pct_count = bytes.iter().filter(|&&b| b == b'%').count();
            match pct_count {
                0 => return value == pattern,
                1 => {
                    if bytes.first() == Some(&b'%') {
                        return value.ends_with(&pattern[1..]);
                    }
                    if bytes.last() == Some(&b'%') {
                        return value.starts_with(&pattern[..pattern.len() - 1]);
                    }
                    // Single `%` in the middle: fall through to DP.
                }
                2 if bytes.first() == Some(&b'%') && bytes.last() == Some(&b'%') => {
                    return value.contains(&pattern[1..pattern.len() - 1]);
                }
                _ => {}
            }
        }

        Self::sql_like_matches_dp(value, pattern)
    }

    fn sql_like_matches_dp(value: &str, pattern: &str) -> bool {
        let value: Vec<char> = value.chars().collect();
        let pattern: Vec<char> = pattern.chars().collect();
        let plen = pattern.len();
        let vlen = value.len();

        // Rolling-row DP: O(plen) extra memory instead of O(vlen * plen).
        let mut prev = vec![false; plen + 1];
        let mut cur = vec![false; plen + 1];
        prev[0] = true;
        for p in 1..=plen {
            if pattern[p - 1] == '%' {
                prev[p] = prev[p - 1];
            }
        }

        for v in 1..=vlen {
            cur[0] = false;
            for p in 1..=plen {
                cur[p] = match pattern[p - 1] {
                    '%' => cur[p - 1] || prev[p],
                    '_' => prev[p - 1],
                    ch => ch == value[v - 1] && prev[p - 1],
                };
            }
            std::mem::swap(&mut prev, &mut cur);
        }

        prev[plen]
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

        // Compute per-column null counts before consuming values (for T8 column stats).
        let null_counts_per_column: Vec<u64> = (0..columns.len())
            .map(|idx| {
                values
                    .iter()
                    .filter(|row| {
                        row.get(idx)
                            .is_none_or(|v| matches!(v, SqlValueLiteral::Null))
                    })
                    .count() as u64
            })
            .collect();

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

        // TD-110 Slice B: enforce PRIMARY KEY uniqueness on INSERT — reject a key that repeats
        // within this statement OR already exists as a committed row. Uses the point `get_by_key`
        // lookup (no full scan). Single-column PK; multi-column PK and non-PK UNIQUE constraints
        // still fail-closed in `CatalogRow::validate` pending the index-backed slice.
        if let Some(pk_column) = Self::primary_key_column(&table_schema) {
            let mut batch_keys: std::collections::HashSet<String> = std::collections::HashSet::new();
            for record in &records {
                let key = record.oid.clone();
                if !batch_keys.insert(key.clone()) {
                    return Err(anyhow!(
                        "duplicate key value violates primary key '{}' on table '{}': '{}' appears more than once in this INSERT",
                        pk_column, table_schema.name, key
                    ));
                }
                let existing = self
                    .record_store
                    .get_by_key(
                        &table_schema,
                        TableRecordGetRequest {
                            table_id: table_id.name.clone(),
                            key: key.clone(),
                            include_vector: false,
                            include_props: false,
                        },
                        None,
                    )
                    .await?;
                if existing.is_some() {
                    return Err(anyhow!(
                        "duplicate key value violates primary key '{}' on table '{}': '{}' already exists",
                        pk_column, table_schema.name, key
                    ));
                }
            }
        }

        // TD-110: enforce non-PK UNIQUE constraints/indexes on INSERT — within-batch
        // dedup + cross-existing probe (O(1) on index-backed stores, Slice C). No
        // rows are excluded: every candidate is a brand-new row.
        let primary_key = Self::primary_key_column(&table_schema);
        let candidate_sets =
            Self::build_unique_candidate_sets(&table_schema, &records, primary_key.as_deref())?;
        if !candidate_sets.is_empty()
            && let Some(conflict) = self
                .record_store
                .check_unique_conflict(
                    &table_schema,
                    &table_id.name,
                    primary_key.as_deref(),
                    &candidate_sets,
                    &std::collections::HashSet::new(),
                    None,
                )
                .await?
        {
            return Err(anyhow!(
                "duplicate key value violates unique constraint on ({}) for table '{}': ({}) already exists",
                conflict.columns.join(", "),
                table_schema.name,
                conflict.tuple.join(", ")
            ));
        }

        // TD-110: enforce FOREIGN KEY references (same-partition cross-table).
        self.enforce_foreign_keys(&table_schema, &records).await?;

        // Compute per-column min/max and NDV from the canonical record props before moving records
        // into mutations. Only orderable types (String, integers) tracked for min/max; floats and
        // booleans are additionally included in the NDV pass.
        let column_minmax = Self::compute_column_minmax_from_records(&records);
        let column_ndv = Self::compute_column_ndv_from_records(&records);

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

        self.bump_row_count_stats(table_name, num_records as i64)
            .await;
        self.bump_column_null_counts(table_name, columns, &null_counts_per_column)
            .await;
        self.bump_column_minmax(table_name, column_minmax).await;
        self.bump_column_ndv(table_name, column_ndv).await;

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
            self.resolve_matching_ids(&table_schema, &table_id.name, wc)
                .await?
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

        // TD-110: enforce non-PK UNIQUE constraints on UPDATE. The rows' NEW
        // values must not collide with OTHER rows (or each other). The updated
        // rows are excluded so a row keeping or vacating its own unique value is
        // not flagged as a self-conflict. (PK is immutable on UPDATE — rejected by
        // validate_update_assignments — so only non-PK UNIQUE sets apply here.)
        let primary_key = Self::primary_key_column(&table_schema);
        let candidate_sets =
            Self::build_unique_candidate_sets(&table_schema, &records, primary_key.as_deref())?;
        if !candidate_sets.is_empty() {
            let exclude_oids: std::collections::HashSet<String> =
                records.iter().map(|record| record.oid.clone()).collect();
            if let Some(conflict) = self
                .record_store
                .check_unique_conflict(
                    &table_schema,
                    &table_id.name,
                    primary_key.as_deref(),
                    &candidate_sets,
                    &exclude_oids,
                    None,
                )
                .await?
            {
                return Err(anyhow!(
                    "duplicate key value violates unique constraint on ({}) for table '{}': ({}) already exists",
                    conflict.columns.join(", "),
                    table_schema.name,
                    conflict.tuple.join(", ")
                ));
            }
        }

        // TD-110: an UPDATE may change a FOREIGN KEY column — re-verify references.
        self.enforce_foreign_keys(&table_schema, &records).await?;

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
            self.resolve_matching_ids(&table_schema, &table_id.name, wc)
                .await?
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

        self.bump_row_count_stats(table_name, -(deleted_count as i64))
            .await;

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

        self.bump_row_count_stats(table_name, num_records as i64)
            .await;

        Ok(
            DmlResult::success(num_records as u64, format!("Upserted {} rows", num_records))
                .with_inserted_ids(inserted_ids),
        )
    }

    // ========================
    // Helper Methods
    // ========================

    /// Current wall-clock time in milliseconds since the Unix epoch, used to mark
    /// `CatalogTableStatistics.last_analyzed_ms` after a stats update so the planner
    /// can detect stale statistics via `CatalogTableStatistics::is_stale`.
    fn now_unix_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as i64
    }

    /// Increment or decrement the row count in catalog statistics after a successful write.
    /// Errors are non-fatal: they are logged as warnings and do not fail the operation.
    /// Called by DML paths and by fast-lane (gRPC/REST/Arrow Flight) write paths for
    /// collections that are registered as relational tables in xCatalog.
    pub(crate) async fn bump_row_count_stats(&self, table_name: &str, delta: i64) {
        let Ok((catalog, table_id)) = self.catalog_manager.resolve_table(table_name).await else {
            return;
        };
        let mut stats = catalog.get_statistics(&table_id).await.unwrap_or_default();
        stats.row_count = if delta >= 0 {
            stats.row_count.saturating_add(delta as u64)
        } else {
            stats.row_count.saturating_sub(delta.unsigned_abs())
        };
        stats.last_analyzed_ms = Some(Self::now_unix_ms());
        if let Err(e) = catalog.update_statistics(&table_id, stats).await {
            warn!(table = %table_name, error = %e, "Failed to update row-count statistics after DML");
        } else {
            Self::bump_corpus_version_for_stats(&table_id).await;
        }
    }

    /// Bump the process-wide corpus_version after a successful stats
    /// refresh so cached planner outputs invalidate against the new
    /// selectivity numbers. Extracts tenant from `namespace[0]` per
    /// the catalog convention; skips when the table isn't
    /// tenant-scoped.
    async fn bump_corpus_version_for_stats(table_id: &crate::catalog::TableIdentifier) {
        let Some(tenant_id) = table_id.namespace.first() else {
            return;
        };
        let version = crate::catalog::CorpusVersionRegistry::global()
            .bump(tenant_id, &table_id.name)
            .await;
        tracing::debug!(
            table = %table_id.name,
            tenant = %tenant_id,
            version,
            "🔄 corpus_version bumped after stats refresh"
        );
    }

    /// Increment per-column null counts in catalog statistics after a successful INSERT.
    /// `null_counts[i]` is the number of NULL values for `columns[i]` in the inserted batch.
    /// Errors are non-fatal. Called only from the INSERT path (null counts grow monotonically).
    async fn bump_column_null_counts(
        &self,
        table_name: &str,
        columns: &[String],
        null_counts: &[u64],
    ) {
        if columns.is_empty() || null_counts.iter().all(|&c| c == 0) {
            return;
        }
        let Ok((catalog, table_id)) = self.catalog_manager.resolve_table(table_name).await else {
            return;
        };
        let mut stats = catalog.get_statistics(&table_id).await.unwrap_or_default();
        for (col_name, &null_delta) in columns.iter().zip(null_counts.iter()) {
            if null_delta > 0 {
                let col_stats = stats.column_stats.entry(col_name.clone()).or_default();
                col_stats.null_count =
                    Some(col_stats.null_count.unwrap_or(0).saturating_add(null_delta));
            }
        }
        stats.last_analyzed_ms = Some(Self::now_unix_ms());
        if let Err(e) = catalog.update_statistics(&table_id, stats).await {
            warn!(table = %table_name, error = %e, "Failed to update column-null statistics after INSERT");
        } else {
            Self::bump_corpus_version_for_stats(&table_id).await;
        }
    }

    /// Convert a `ProximaValue` to a lexicographically sortable string for min/max tracking.
    /// Returns `None` for types without natural total order (Null, Float, Binary, Vector, etc.).
    fn value_to_minmax_string(value: &ProximaValue) -> Option<String> {
        match value {
            // Integers: zero-padded with explicit sign for correct lex order.
            ProximaValue::Int8(v) => Some(format!("{:+020}", *v as i64)),
            ProximaValue::Int16(v) => Some(format!("{:+020}", *v as i64)),
            ProximaValue::Int32(v) => Some(format!("{:+020}", *v as i64)),
            ProximaValue::Int64(v) => Some(format!("{:+020}", v)),
            // Strings: use directly (lexicographic order is natural for text columns).
            ProximaValue::String(s) => Some(s.clone()),
            // Dates/timestamps stored in ISO 8601 are naturally sortable.
            ProximaValue::Date(d) => Some(format!("{d}")),
            // Floats, Null, Binary, Vector, Struct, etc.: skip.
            _ => None,
        }
    }

    /// Compute per-column min/max sortable strings from a record batch before the records
    /// are consumed. Returns a map of column → (min_string, max_string).
    fn compute_column_minmax_from_records(
        records: &[ProximaRecord],
    ) -> HashMap<String, (String, String)> {
        let mut minmax: HashMap<String, (String, String)> = HashMap::new();
        for record in records {
            for (col, node) in record.props.iter() {
                let ProximaTreeNode::Value(val) = node else {
                    continue;
                };
                let Some(s) = Self::value_to_minmax_string(val) else {
                    continue;
                };
                match minmax.get_mut(col) {
                    Some((min, max)) => {
                        if s < *min {
                            *min = s.clone();
                        }
                        if s > *max {
                            *max = s;
                        }
                    }
                    None => {
                        minmax.insert(col.clone(), (s.clone(), s));
                    }
                }
            }
        }
        minmax
    }

    /// Merge per-column batch min/max into `CatalogTableStatistics.column_stats[col].min_value`
    /// / `max_value`. Errors are non-fatal (logged as warnings).
    async fn bump_column_minmax(
        &self,
        table_name: &str,
        column_minmax: HashMap<String, (String, String)>,
    ) {
        if column_minmax.is_empty() {
            return;
        }
        let Ok((catalog, table_id)) = self.catalog_manager.resolve_table(table_name).await else {
            return;
        };
        let mut stats = catalog.get_statistics(&table_id).await.unwrap_or_default();
        for (col, (batch_min, batch_max)) in column_minmax {
            let col_stats = stats.column_stats.entry(col).or_default();
            col_stats.min_value = Some(match col_stats.min_value.take() {
                Some(existing) if existing <= batch_min => existing,
                _ => batch_min,
            });
            col_stats.max_value = Some(match col_stats.max_value.take() {
                Some(existing) if existing >= batch_max => existing,
                _ => batch_max,
            });
        }
        stats.last_analyzed_ms = Some(Self::now_unix_ms());
        if let Err(e) = catalog.update_statistics(&table_id, stats).await {
            warn!(table = %table_name, error = %e, "Failed to update column min/max statistics after INSERT");
        } else {
            Self::bump_corpus_version_for_stats(&table_id).await;
        }
    }

    /// Convert a `ProximaValue` to a string key suitable for distinct-value counting.
    /// Covers a wider set than `value_to_minmax_string` (includes Float, Bool).
    fn value_to_ndv_key(value: &ProximaValue) -> Option<String> {
        match value {
            ProximaValue::Int8(v) => Some(format!("{v}")),
            ProximaValue::Int16(v) => Some(format!("{v}")),
            ProximaValue::Int32(v) => Some(format!("{v}")),
            ProximaValue::Int64(v) => Some(format!("{v}")),
            ProximaValue::Float32(v) => Some(format!("{v}")),
            ProximaValue::Float64(v) => Some(format!("{v}")),
            ProximaValue::String(s) => Some(s.clone()),
            ProximaValue::Boolean(b) => Some(if *b { "1" } else { "0" }.to_string()),
            ProximaValue::Date(d) => Some(format!("{d}")),
            _ => None,
        }
    }

    /// Count distinct values per column in a batch.
    /// Returns a map of column → distinct count within this batch.
    fn compute_column_ndv_from_records(records: &[ProximaRecord]) -> HashMap<String, u64> {
        let mut seen: HashMap<String, std::collections::HashSet<String>> = HashMap::new();
        for record in records {
            for (col, node) in record.props.iter() {
                let ProximaTreeNode::Value(val) = node else {
                    continue;
                };
                let Some(key) = Self::value_to_ndv_key(val) else {
                    continue;
                };
                seen.entry(col.clone()).or_default().insert(key);
            }
        }
        seen.into_iter()
            .map(|(col, set)| (col, set.len() as u64))
            .collect()
    }

    /// Merge per-column distinct counts from a batch into `CatalogColumnStatistics.distinct_count`.
    ///
    /// Uses an additive estimate (assumes no overlap between batches), capped at the current
    /// table row count. This matches how simple columnar stats systems seed NDV before ANALYZE.
    /// Errors are non-fatal (logged as warnings).
    async fn bump_column_ndv(&self, table_name: &str, ndv_per_column: HashMap<String, u64>) {
        if ndv_per_column.is_empty() {
            return;
        }
        let Ok((catalog, table_id)) = self.catalog_manager.resolve_table(table_name).await else {
            return;
        };
        let mut stats = catalog.get_statistics(&table_id).await.unwrap_or_default();
        let row_count_cap = stats.row_count.max(1);
        for (col, batch_ndv) in ndv_per_column {
            let col_stats = stats.column_stats.entry(col).or_default();
            let new_ndv = col_stats
                .distinct_count
                .unwrap_or(0)
                .saturating_add(batch_ndv);
            col_stats.distinct_count = Some(new_ndv.min(row_count_cap));
        }
        stats.last_analyzed_ms = Some(Self::now_unix_ms());
        if let Err(e) = catalog.update_statistics(&table_id, stats).await {
            warn!(table = %table_name, error = %e, "Failed to update column NDV statistics after INSERT");
        } else {
            Self::bump_corpus_version_for_stats(&table_id).await;
        }
    }

    /// Validate a batch of `ProximaRecord`s against the xCatalog schema for `collection_name`.
    ///
    /// Designed for fast-lane REST/gRPC/Arrow Flight writes that arrive as pre-built records
    /// rather than SQL literals. Silently skips validation if the collection is not registered
    /// as a relational table (non-relational collections remain unrestricted). Returns `Err`
    /// containing the column constraint violation message on the first failing record.
    pub async fn validate_record_batch_against_schema(
        &self,
        collection_name: &str,
        records: &[ProximaRecord],
    ) -> Result<()> {
        let Ok((catalog, table_id)) = self.catalog_manager.resolve_table(collection_name).await
        else {
            return Ok(());
        };
        let Ok(table_schema) = catalog.get_table(&table_id).await else {
            return Ok(());
        };
        // v2 vector record API path: when the caller provides at least one
        // embedding cell on every record, skip relational schema validation.
        // The v2 records/batch endpoint is the vector ingest surface, not a
        // SQL DML surface. Relational schema constraints (`reject_unknown_columns`,
        // missing-required-column, type strictness) reject perfectly valid
        // vector-API batches — including ones that carry filter metadata in
        // `props` — because the auto-registered schema is `id` + `vector`
        // only and treats anything else as unknown. Reconciled 2026-05-28 for
        // the v0.2 v2 INSERT→SEARCH gap.
        let all_records_are_vector_shaped =
            !records.is_empty() && records.iter().all(|r| !r.embeddings.is_empty());
        if all_records_are_vector_shaped {
            return Ok(());
        }
        // Determine which schema column (if any) maps to the record's canonical
        // identifier (`oid`). Auto-registered vector-collection schemas declare
        // either `id` or `record_id` for this — line up with the same convention
        // used in `dml_field_mapping` below. Without this projection, REST/gRPC
        // v2 INSERT records (whose OID lives outside `props`) fail validation
        // with "Missing required column 'id'" even though the OID was provided.
        let id_column_name: Option<String> = table_schema
            .columns
            .iter()
            .find(|c| c.name == "id" || c.name == "record_id")
            .map(|c| c.name.clone());
        let profile = RelationalWriteProfile::fast_lane();
        for record in records {
            let mut values: HashMap<String, ProximaValue> = record
                .props
                .iter()
                .filter_map(|(k, node)| {
                    if let proximadb_records::ProximaTreeNode::Value(v) = node {
                        Some((k.clone(), v.clone()))
                    } else {
                        None
                    }
                })
                .collect();
            if let Some(id_col) = id_column_name.as_ref() {
                values
                    .entry(id_col.clone())
                    .or_insert_with(|| ProximaValue::String(record.oid.clone()));
            }
            CatalogRow::validate(&table_schema, values, &profile).with_context(|| {
                format!(
                    "record '{}' violates schema '{}'",
                    record.oid, collection_name
                )
            })?;
        }
        Ok(())
    }

    /// Build a canonical ProximaRecord from catalog schema and SQL literals.
    fn build_mutation_record(
        &self,
        columns: &[String],
        values: &[SqlValueLiteral],
        table_schema: &CatalogTableSchema,
        mutation_kind: RelationalMutationKind,
    ) -> Result<ProximaRecord> {
        // `INSERT INTO t VALUES (...)` without an explicit column
        // list is standard SQL — the values are mapped to the table's
        // declared columns in order. Synthesize the column list
        // here when it's empty so the validation + per-column
        // mapping below behaves uniformly. The arity check then
        // catches actual mismatches (too many/few values vs the
        // schema's column count) with a clearer error.
        let synthesized: Vec<String>;
        let columns: &[String] = if columns.is_empty() && !values.is_empty() {
            synthesized = table_schema
                .columns
                .iter()
                .map(|c| c.name.clone())
                .collect();
            &synthesized
        } else {
            columns
        };
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

        match &column.data_type {
            ProximaType::Int8 => key_value
                .parse::<i8>()
                .map(ProximaValue::Int8)
                .map_err(|e| anyhow!("Invalid primary key value '{}': {}", key_value, e)),
            ProximaType::Int16 => key_value
                .parse::<i16>()
                .map(ProximaValue::Int16)
                .map_err(|e| anyhow!("Invalid primary key value '{}': {}", key_value, e)),
            ProximaType::Int32 => key_value
                .parse::<i32>()
                .map(ProximaValue::Int32)
                .map_err(|e| anyhow!("Invalid primary key value '{}': {}", key_value, e)),
            ProximaType::Int64 => key_value
                .parse::<i64>()
                .map(ProximaValue::Int64)
                .map_err(|e| anyhow!("Invalid primary key value '{}': {}", key_value, e)),
            ProximaType::Uuid => Ok(ProximaValue::String(key_value.to_string())),
            ProximaType::String => Ok(ProximaValue::String(key_value.to_string())),
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

            if column_name == "timestamp"
                && let Some(timestamp_ms) = self.literal_to_timestamp(&effective_value)?
            {
                let timestamp_ns = timestamp_ms.saturating_mul(1_000_000);
                created_at_ns = Some(timestamp_ns);
                updated_at_ns = timestamp_ns;
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

            if matches!(column.data_type, ProximaType::DenseVector { .. }) {
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
                        match &column.data_type {
                            ProximaType::TimestampTz(_) => ProximaValue::TimestampTz(
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
        // Shared with the record-store index (TD-110 Slice C) so candidate and
        // indexed tuples derive their PK column identically.
        crate::services::record_store::schema_primary_key_column(table_schema)
    }

    /// TD-110: the column sets that carry a UNIQUE guarantee — cataloged unique
    /// indexes plus inline `UNIQUE (...)` column constraints. Delegates to the
    /// shared store-layer helper so DmlService candidates and the store's index
    /// enforce exactly the same sets.
    fn unique_column_sets(table_schema: &CatalogTableSchema) -> Vec<Vec<String>> {
        crate::services::record_store::schema_unique_column_sets(table_schema)
    }

    /// TD-110: build the per-set candidate tuples for `records`, rejecting a tuple
    /// that repeats within this statement (NULL tuples exempt). Shared by INSERT
    /// and UPDATE; the caller passes the result to `check_unique_conflict`.
    fn build_unique_candidate_sets(
        table_schema: &CatalogTableSchema,
        records: &[ProximaRecord],
        primary_key: Option<&str>,
    ) -> Result<Vec<crate::services::record_store::UniqueCandidateSet>> {
        let unique_sets = Self::unique_column_sets(table_schema);
        let mut candidate_sets = Vec::with_capacity(unique_sets.len());
        for columns in &unique_sets {
            let mut candidates: std::collections::HashSet<Vec<String>> =
                std::collections::HashSet::new();
            for record in records {
                let Some(tuple) = record_unique_tuple(record, columns, primary_key) else {
                    continue; // NULL/absent in the tuple → exempt
                };
                if !candidates.insert(tuple.clone()) {
                    return Err(anyhow!(
                        "duplicate key value violates unique constraint on ({}) for table '{}': ({}) appears more than once in this statement",
                        columns.join(", "),
                        table_schema.name,
                        tuple.join(", ")
                    ));
                }
            }
            if !candidates.is_empty() {
                candidate_sets.push(crate::services::record_store::UniqueCandidateSet {
                    columns: columns.clone(),
                    candidates,
                });
            }
        }
        Ok(candidate_sets)
    }

    /// TD-110: enforce FOREIGN KEY references for `records` against parent tables
    /// in the same partition (cross-table state the row-local catalog validator
    /// cannot check). Supported shape: a single-column FK referencing the parent
    /// PRIMARY KEY — verified by a point `get_by_key` on the parent. NULL FK
    /// values are exempt. Unsupported shapes (composite FK, or a referenced
    /// column that is not the parent PK) are cleanly rejected rather than
    /// silently accepted.
    async fn enforce_foreign_keys(
        &self,
        table_schema: &CatalogTableSchema,
        records: &[ProximaRecord],
    ) -> Result<()> {
        let child_primary_key = Self::primary_key_column(table_schema);
        for constraint in &table_schema.relational_capabilities.constraints {
            let proximadb_catalog::ColumnConstraint::ForeignKey {
                columns,
                references_table,
                references_columns,
                ..
            } = constraint
            else {
                continue;
            };
            if columns.len() != 1 || references_columns.len() != 1 {
                return Err(anyhow!(
                    "composite FOREIGN KEY ({}) on table '{}' is not supported yet",
                    columns.join(", "),
                    table_schema.name
                ));
            }
            let fk_column = &columns[0];
            let referenced_column = &references_columns[0];

            let (parent_catalog, parent_table_id) = self
                .catalog_manager
                .resolve_table(references_table)
                .await
                .map_err(|err| {
                    anyhow!(
                        "FOREIGN KEY ({}) on table '{}' references table '{}' which cannot be resolved: {err}",
                        fk_column, table_schema.name, references_table
                    )
                })?;
            if !parent_catalog.table_exists(&parent_table_id).await? {
                return Err(anyhow!(
                    "FOREIGN KEY ({}) on table '{}' references missing table '{}'",
                    fk_column,
                    table_schema.name,
                    references_table
                ));
            }
            let parent_schema = parent_catalog.get_table(&parent_table_id).await?;
            if Self::primary_key_column(&parent_schema).as_deref() != Some(referenced_column.as_str())
            {
                return Err(anyhow!(
                    "FOREIGN KEY ({}) REFERENCES {}({}) on table '{}' is only supported when it references the parent primary key",
                    fk_column, references_table, referenced_column, table_schema.name
                ));
            }

            for record in records {
                let Some(values) = record_unique_tuple(
                    record,
                    std::slice::from_ref(fk_column),
                    child_primary_key.as_deref(),
                ) else {
                    continue; // NULL/absent FK → no reference required
                };
                let key = values.into_iter().next().unwrap_or_default();
                let referenced_exists = self
                    .record_store
                    .get_by_key(
                        &parent_schema,
                        TableRecordGetRequest {
                            table_id: parent_table_id.name.clone(),
                            key: key.clone(),
                            include_vector: false,
                            include_props: false,
                        },
                        None,
                    )
                    .await?
                    .is_some();
                if !referenced_exists {
                    return Err(anyhow!(
                        "FOREIGN KEY ({}) on table '{}' violates reference: '{}' is not present in {}({})",
                        fk_column, table_schema.name, key, references_table, referenced_column
                    ));
                }
            }
        }
        Ok(())
    }

    /// Extract IDs from WHERE clause using the catalog primary key.
    /// Resolve the canonical OIDs of the rows an UPDATE/DELETE `WHERE` clause
    /// targets, honoring the FULL predicate (any catalog column), not just the
    /// primary key. Reuses the same predicate evaluator + scan push-down as
    /// SELECT, so mixed `pk = x AND col = y` no longer mutates rows that fail the
    /// non-PK part (the prior silent bug), and non-PK `WHERE` now works.
    async fn resolve_matching_ids(
        &self,
        table_schema: &CatalogTableSchema,
        table_id_name: &str,
        where_clause: &WhereClause,
    ) -> Result<Vec<String>> {
        let tree = self.where_clause_to_predicate_tree(where_clause, table_schema)?;
        let primary_key = table_schema.primary_key.first().map(String::as_str);

        // PK fast-path: when the WHERE pins primary-key `=`/`IN` candidates (only
        // sound under a top-level conjunction — see `extract_pk_candidate_ids`),
        // fetch them by key and keep only those that ALSO satisfy the full tree.
        let pk_candidates = self.extract_pk_candidate_ids(where_clause, table_schema)?;
        if !pk_candidates.is_empty() {
            let mut ids = Vec::new();
            for candidate in pk_candidates {
                let Some(rich) = self
                    .record_store
                    .get_by_key(
                        table_schema,
                        TableRecordGetRequest {
                            table_id: table_id_name.to_string(),
                            key: candidate,
                            include_vector: true,
                            include_props: true,
                        },
                        None,
                    )
                    .await?
                else {
                    continue;
                };
                let record = Self::rich_result_to_record(rich);
                if Self::eval_predicate_tree(&record, &tree, primary_key) {
                    ids.push(record.oid);
                }
            }
            return Ok(ids);
        }

        // No PK predicate: push the full predicate tree into the store scan.
        let pred = |record: &ProximaRecord| Self::eval_predicate_tree(record, &tree, primary_key);
        let predicate: Option<&proximadb_records::RecordScanPredicate<'_>> = Some(&pred);
        let records = self
            .record_store
            .scan_records_filtered(
                table_schema,
                TableRecordScanRequest {
                    table_id: table_id_name.to_string(),
                    limit: None,
                    include_vector: true,
                    include_props: true,
                },
                predicate,
                None,
            )
            .await?;
        Ok(records.into_iter().map(|record| record.oid).collect())
    }

    /// Extract primary-key `=`/`IN` candidate OIDs from a WHERE clause. Returns
    /// an empty Vec (NOT an error) when the clause has no usable PK predicate, so
    /// the caller can fall back to a predicate scan. Candidates are still
    /// re-checked against the full predicate by [`Self::resolve_matching_ids`].
    fn extract_pk_candidate_ids(
        &self,
        where_clause: &WhereClause,
        table_schema: &CatalogTableSchema,
    ) -> Result<Vec<String>> {
        // Only sound under a top-level conjunction: under OR a PK leaf does not
        // bound the match set (`id = 5 OR status = 'x'` matches any id), so force
        // the full scan path. A PK leaf nested under an inner OR yields no
        // top-level candidate below → also scans.
        if matches!(where_clause.operator, LogicalOperator::Or) && where_clause.conditions.len() > 1
        {
            return Ok(Vec::new());
        }
        let Some(primary_key_column) = Self::primary_key_column(table_schema) else {
            return Ok(Vec::new());
        };
        let mut ids = Vec::new();
        for condition in &where_clause.conditions {
            match condition {
                Condition::Comparison {
                    column,
                    operator,
                    value,
                } if column == &primary_key_column
                    && matches!(operator, ComparisonOperator::Equal) =>
                {
                    ids.push(self.literal_to_string(value)?);
                }
                Condition::In {
                    column,
                    values,
                    negated,
                } if column == &primary_key_column && !negated => {
                    for v in values {
                        ids.push(self.literal_to_string(v)?);
                    }
                }
                _ => {}
            }
        }
        Ok(ids)
    }

    /// Convert an UPDATE/DELETE `WhereClause` into a resolved boolean predicate
    /// tree. Columns are resolved against the catalog ONCE here (so per-record
    /// evaluation is infallible); supports AND/OR/nested/BETWEEN/NOT BETWEEN.
    fn where_clause_to_predicate_tree(
        &self,
        where_clause: &WhereClause,
        table_schema: &CatalogTableSchema,
    ) -> Result<RelationalPredicateTree> {
        self.combine_conditions(
            &where_clause.conditions,
            where_clause.operator,
            table_schema,
        )
    }

    /// Combine a list of conditions with the given logical operator.
    fn combine_conditions(
        &self,
        conditions: &[Condition],
        operator: LogicalOperator,
        table_schema: &CatalogTableSchema,
    ) -> Result<RelationalPredicateTree> {
        if conditions.is_empty() {
            return Err(anyhow!("WHERE clause has no conditions for UPDATE/DELETE"));
        }
        let mut children = conditions
            .iter()
            .map(|condition| self.condition_to_predicate_tree(condition, table_schema))
            .collect::<Result<Vec<_>>>()?;
        Ok(if children.len() == 1 {
            children.pop().expect("len == 1")
        } else {
            match operator {
                LogicalOperator::And => RelationalPredicateTree::And(children),
                LogicalOperator::Or => RelationalPredicateTree::Or(children),
            }
        })
    }

    /// Lower one `Condition` into a resolved predicate tree node. `BETWEEN`
    /// expands to `>= low AND <= high` (negated → `Not`); `Nested` recurses.
    fn condition_to_predicate_tree(
        &self,
        condition: &Condition,
        table_schema: &CatalogTableSchema,
    ) -> Result<RelationalPredicateTree> {
        // Resolve a single column-leaf condition into a tree `Leaf`.
        let leaf = |column: &str,
                    cond: RelationalSelectPredicateCondition|
         -> Result<RelationalPredicateTree> {
            let resolved = Self::resolve_select_predicates(
                table_schema,
                &[RelationalSelectPredicateInput {
                    column_name: column.to_string(),
                    condition: cond,
                }],
            )?;
            Ok(RelationalPredicateTree::Leaf(
                resolved
                    .into_iter()
                    .next()
                    .expect("one input → one predicate"),
            ))
        };
        match condition {
            Condition::Comparison {
                column,
                operator,
                value,
            } => leaf(
                column,
                RelationalSelectPredicateCondition::Comparison {
                    operator: Self::map_comparison_operator(*operator),
                    literal: self.literal_to_string(value)?,
                },
            ),
            Condition::In {
                column,
                values,
                negated,
            } => {
                let literals = values
                    .iter()
                    .map(|value| self.literal_to_string(value))
                    .collect::<Result<Vec<_>>>()?;
                leaf(
                    column,
                    RelationalSelectPredicateCondition::In {
                        literals,
                        negated: *negated,
                    },
                )
            }
            Condition::Like {
                column,
                pattern,
                negated,
            } => leaf(
                column,
                RelationalSelectPredicateCondition::Like {
                    pattern: pattern.clone(),
                    negated: *negated,
                },
            ),
            Condition::IsNull { column, negated } => leaf(
                column,
                RelationalSelectPredicateCondition::IsNull { negated: *negated },
            ),
            Condition::Between {
                column,
                low,
                high,
                negated,
            } => {
                // col BETWEEN low AND high  ≡  col >= low AND col <= high
                let ge = leaf(
                    column,
                    RelationalSelectPredicateCondition::Comparison {
                        operator: RelationalSelectPredicateOperator::GreaterThanOrEqual,
                        literal: self.literal_to_string(low)?,
                    },
                )?;
                let le = leaf(
                    column,
                    RelationalSelectPredicateCondition::Comparison {
                        operator: RelationalSelectPredicateOperator::LessThanOrEqual,
                        literal: self.literal_to_string(high)?,
                    },
                )?;
                let between = RelationalPredicateTree::And(vec![ge, le]);
                Ok(if *negated {
                    RelationalPredicateTree::Not(Box::new(between))
                } else {
                    between
                })
            }
            Condition::Nested {
                conditions,
                operator,
            } => self.combine_conditions(conditions, *operator, table_schema),
        }
    }

    fn map_comparison_operator(operator: ComparisonOperator) -> RelationalSelectPredicateOperator {
        match operator {
            ComparisonOperator::Equal => RelationalSelectPredicateOperator::Equal,
            ComparisonOperator::NotEqual => RelationalSelectPredicateOperator::NotEqual,
            ComparisonOperator::LessThan => RelationalSelectPredicateOperator::LessThan,
            ComparisonOperator::LessThanOrEqual => {
                RelationalSelectPredicateOperator::LessThanOrEqual
            }
            ComparisonOperator::GreaterThan => RelationalSelectPredicateOperator::GreaterThan,
            ComparisonOperator::GreaterThanOrEqual => {
                RelationalSelectPredicateOperator::GreaterThanOrEqual
            }
        }
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
            if (value.starts_with('{') || value.starts_with('['))
                && let Ok(json) = serde_json::from_str(&value)
            {
                return Ok(SqlValueLiteral::Json(json));
            }
            return Ok(SqlValueLiteral::String(value));
        }

        if let Ok(value) = trimmed.parse::<i64>() {
            return Ok(SqlValueLiteral::Integer(value));
        }
        if let Ok(value) = trimmed.parse::<f64>() {
            return Ok(SqlValueLiteral::Float(value));
        }
        if (trimmed.starts_with('{') || trimmed.starts_with('['))
            && let Ok(json) = serde_json::from_str(trimmed)
        {
            return Ok(SqlValueLiteral::Json(json));
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

        match &column.data_type {
            ProximaType::Boolean => match val {
                SqlValueLiteral::Boolean(value) => Ok(ProximaValue::Boolean(*value)),
                SqlValueLiteral::Null if column.nullable => Ok(ProximaValue::Null),
                _ => Err(anyhow!("Column '{}' expects boolean", column_name)),
            },
            ProximaType::Int8 => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    return Ok(ProximaValue::Null);
                }
                self.literal_to_i64(val)
                    .map(|v| ProximaValue::Int8(v as i8))
            }
            ProximaType::Int16 => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    return Ok(ProximaValue::Null);
                }
                self.literal_to_i64(val)
                    .map(|v| ProximaValue::Int16(v as i16))
            }
            ProximaType::Int32 => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    return Ok(ProximaValue::Null);
                }
                self.literal_to_i64(val)
                    .map(|v| ProximaValue::Int32(v as i32))
            }
            ProximaType::Int64 => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    return Ok(ProximaValue::Null);
                }
                self.literal_to_i64(val).map(ProximaValue::Int64)
            }
            ProximaType::Float32 => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    return Ok(ProximaValue::Null);
                }
                self.literal_to_f64(val)
                    .map(|v| ProximaValue::Float32(v as f32))
            }
            ProximaType::Float64 => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    return Ok(ProximaValue::Null);
                }
                self.literal_to_f64(val).map(ProximaValue::Float64)
            }
            ProximaType::String | ProximaType::Uuid => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    return Ok(ProximaValue::Null);
                }
                self.literal_to_string(val).map(ProximaValue::String)
            }
            ProximaType::Json => {
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
            ProximaType::DenseVector { .. } => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    Ok(ProximaValue::Null)
                } else {
                    self.literal_to_vector(val).map(ProximaValue::DenseVector)
                }
            }
            ProximaType::Binary | ProximaType::BinaryVector { .. } => match val {
                SqlValueLiteral::Binary(value) => Ok(ProximaValue::Binary(value.clone())),
                SqlValueLiteral::Null if column.nullable => Ok(ProximaValue::Null),
                _ => Err(anyhow!("Column '{}' expects binary", column_name)),
            },
            ProximaType::Date => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    return Ok(ProximaValue::Null);
                }
                self.literal_to_i64(val)
                    .map(|value| ProximaValue::Date(value as i32))
            }
            ProximaType::Time(_) => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    return Ok(ProximaValue::Null);
                }
                self.literal_to_i64(val).map(|value| {
                    ProximaValue::Time(value, proximadb_data_model::TimeUnit::Millisecond)
                })
            }
            ProximaType::Timestamp(_) => self.literal_to_timestamp(val).map(|value| {
                value
                    .map(|timestamp| {
                        ProximaValue::Timestamp(
                            timestamp,
                            proximadb_data_model::TimeUnit::Millisecond,
                        )
                    })
                    .unwrap_or(ProximaValue::Null)
            }),
            ProximaType::TimestampTz(_) => self.literal_to_timestamp(val).map(|value| {
                value
                    .map(|timestamp| {
                        ProximaValue::TimestampTz(
                            timestamp,
                            proximadb_data_model::TimeUnit::Millisecond,
                        )
                    })
                    .unwrap_or(ProximaValue::Null)
            }),
            ProximaType::Decimal { .. } => {
                if matches!(val, SqlValueLiteral::Null) && column.nullable {
                    return Ok(ProximaValue::Null);
                }
                self.literal_to_string(val).map(ProximaValue::Decimal)
            }
            ProximaType::SparseVector { .. } => Err(anyhow!(
                "Sparse vector DML literal lowering is not implemented for column '{}'",
                column_name
            )),
            // Richer ProximaType variants (unsigned ints, Float16, Symbol,
            // Jsonb, Array, Map, Struct, Interval, Duration, ULID, geo, Null)
            // are not produced by the SQL/catalog type surface; reject DML
            // literal lowering for them rather than silently coercing.
            other => Err(anyhow!(
                "Column '{}' has type {:?} which is not supported for DML literal lowering",
                column_name,
                other
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
    use proximadb_catalog::{
        CatalogColumn, CatalogProjection, CatalogProjectionKind, CatalogStorageSpecialization,
        CatalogWorkloadProfile,
    };
    use proximadb_iceberg_engine::IcebergObjectStoreBridge;

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
            .with_column(CatalogColumn::new(1, "record_id", ProximaType::String).nullable(false))
            .with_column(CatalogColumn::new(2, "name", ProximaType::String).nullable(false))
            .with_column(
                CatalogColumn::new(3, "payload", ProximaType::Json).with_default("'{}'::jsonb"),
            )
            .with_column(CatalogColumn::new(4, "notes", ProximaType::String))
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
            .with_column(CatalogColumn::new(1, "id", ProximaType::Int32))
            .with_column(CatalogColumn::new(2, "name", ProximaType::String))
            .with_column(CatalogColumn::new(3, "active", ProximaType::Boolean))
            .with_column(CatalogColumn::new(
                4,
                "embedding",
                ProximaType::DenseVector {
                    element: proximadb_data_model::VectorElement::Float32,
                    dim: 0,
                },
            ))
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
                values: proximadb_records::EmbeddingValues::Fp32(vec![0.1, 0.2]),
                ..Default::default()
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
            .with_column(CatalogColumn::new(1, "id", ProximaType::Int32))
            .with_column(CatalogColumn::new(2, "name", ProximaType::String))
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
            predicates.len(),
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
        let statement = parser
            .parse_ddl("CREATE TABLE staging (id TEXT NOT NULL, payload JSONB);")
            .expect("parse ddl")
            .expect("ddl statement");
        ddl.execute(statement).await.expect("execute ddl");

        let mut facts_schema = CatalogTableSchema::new("facts")
            .with_column(CatalogColumn::new(1, "id", ProximaType::String).nullable(false))
            .with_column(CatalogColumn::new(2, "payload", ProximaType::Json))
            .with_workload_profile(CatalogWorkloadProfile::Olap)
            .with_storage_specialization(CatalogStorageSpecialization::ColumnarAnalytics)
            .with_projection(
                CatalogProjection::rebuildable(
                    "facts_iceberg_publication",
                    CatalogProjectionKind::Columnar,
                    "primary",
                )
                .with_bounded_lag(5_000)
                .with_lineage("wal:1..42", "wal:42")
                .with_policy_and_gate("engine-enforced", "projection-publication-smoke"),
            );
        facts_schema
            .properties
            .insert("compute_route".to_string(), "datafusion-local".to_string());
        facts_schema
            .properties
            .insert("freshness_sla".to_string(), "5s".to_string());

        let (catalog, table_id) = manager.resolve_table("facts").await.expect("resolve facts");
        catalog
            .create_table(&table_id, facts_schema)
            .await
            .expect("create facts schema with projection metadata");

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
        assert_eq!(
            explanation
                .route_metadata
                .projection_freshness_state
                .as_deref(),
            Some("Fresh")
        );
        assert_eq!(explanation.route_metadata.projection_metadata.len(), 1);
        let projection = &explanation.route_metadata.projection_metadata[0];
        assert_eq!(projection.name, "facts_iceberg_publication");
        assert_eq!(projection.kind, "Columnar");
        assert_eq!(projection.rebuild_source, "primary");
        assert_eq!(projection.freshness, "BoundedLag");
        assert_eq!(projection.freshness_state, "Fresh");
        assert_eq!(projection.max_lag_ms, Some(5_000));
        assert_eq!(projection.source_range.as_deref(), Some("wal:1..42"));
        assert_eq!(projection.last_included_position.as_deref(), Some("wal:42"));
        assert_eq!(
            projection.policy_boundary.as_deref(),
            Some("engine-enforced")
        );
        assert_eq!(
            projection.benchmark_gate.as_deref(),
            Some("projection-publication-smoke")
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

    #[tokio::test]
    async fn object_store_bridge_insert_select_executes_through_datafusion_route() {
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
            "CREATE TABLE staging (id TEXT NOT NULL, amount INTEGER NOT NULL, PRIMARY KEY (id));",
            "CREATE TABLE facts (id TEXT NOT NULL, amount INTEGER NOT NULL, PRIMARY KEY (id))
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

        let bridge: Arc<dyn ObjectStoreBridge> =
            Arc::new(IcebergObjectStoreBridge::from_url("memory://").expect("object bridge"));
        let iceberg_store: Arc<dyn TableRecordStore> =
            Arc::new(ObjectStoreIcebergRecordStore::new(bridge.clone()));
        let vector_store: Arc<dyn TableRecordStore> =
            Arc::new(ObjectStoreVectorRecordStore::new(bridge.clone()));
        let routed_store: Arc<dyn TableRecordStore> = Arc::new(
            CatalogRoutingTableRecordStore::new(iceberg_store, vector_store.clone(), vector_store),
        );
        let source_reader = Arc::new(TableRecordStoreSourceReader::new(routed_store.clone()));
        let table_write_executor = Arc::new(
            DataFusionTableWriteExecutor::new(source_reader, routed_store.clone())
                .with_object_store_bridge(bridge),
        );
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager,
            routed_store,
            table_write_executor,
        );

        let insert = dml
            .execute(DmlStatement::Insert {
                table_name: "staging".to_string(),
                columns: vec!["id".to_string(), "amount".to_string()],
                values: vec![
                    vec![
                        SqlValueLiteral::String("s1".to_string()),
                        SqlValueLiteral::Integer(42),
                    ],
                    vec![
                        SqlValueLiteral::String("s2".to_string()),
                        SqlValueLiteral::Integer(77),
                    ],
                ],
            })
            .await
            .expect("insert source rows");
        assert_eq!(insert.rows_affected, 2);

        let statement = parser
            .parse_dml("INSERT INTO facts SELECT * FROM staging;")
            .expect("parse dml")
            .expect("dml statement");
        let copy = dml.execute(statement).await.expect("execute insert select");

        assert_eq!(copy.rows_affected, 2);
        assert!(copy.message.contains("DataFusionLocal"));

        let (_schema, mut records) = dml
            .scan_table_records("facts", None)
            .await
            .expect("scan target rows");
        records.sort_by(|left, right| left.oid.cmp(&right.oid));
        assert_eq!(
            records
                .iter()
                .map(|record| record.oid.as_str())
                .collect::<Vec<_>>(),
            vec!["s1", "s2"]
        );
        let amounts = records
            .iter()
            .map(|record| proximadb_records::tree_get(&record.props, "amount"))
            .collect::<Vec<_>>();
        assert_eq!(
            amounts,
            vec![
                Some(&ProximaValue::Int32(42)),
                Some(&ProximaValue::Int32(77))
            ]
        );
    }

    #[tokio::test]
    async fn explain_insert_values_returns_native_oltp_route() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl("CREATE TABLE orders (id TEXT NOT NULL, amount FLOAT);")
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(ExplainOnlyRecordStore),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        let stmt = parser
            .parse_dml("INSERT INTO orders (id, amount) VALUES ('r1', 9.99);")
            .expect("parse dml")
            .expect("dml stmt");

        let explanation = dml
            .explain_table_write(stmt)
            .await
            .expect("explain values insert");

        assert_eq!(explanation.target_table, "orders");
        assert_eq!(explanation.selected_backend, "Native");
        // Default table (no WITH options) gets the htap workload profile.
        assert!(
            explanation.route_metadata.workload_profile == "htap"
                || explanation.route_metadata.workload_profile == "oltp",
            "unexpected workload_profile: {}",
            explanation.route_metadata.workload_profile
        );
        assert!(
            explanation.write_lane.contains("Wal"),
            "expected WAL lane, got {:?}",
            explanation.write_lane
        );
    }

    #[tokio::test]
    async fn explain_update_and_delete_return_routes() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl("CREATE TABLE accounts (id TEXT NOT NULL, balance FLOAT);")
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(ExplainOnlyRecordStore),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        let update_stmt = parser
            .parse_dml("UPDATE accounts SET balance = 100.0 WHERE id = 'a1';")
            .expect("parse dml")
            .expect("update stmt");
        let update_explanation = dml
            .explain_table_write(update_stmt)
            .await
            .expect("explain update");
        assert_eq!(update_explanation.target_table, "accounts");
        assert_eq!(update_explanation.selected_backend, "Native");

        let delete_stmt = parser
            .parse_dml("DELETE FROM accounts WHERE id = 'a1';")
            .expect("parse dml")
            .expect("delete stmt");
        let delete_explanation = dml
            .explain_table_write(delete_stmt)
            .await
            .expect("explain delete");
        assert_eq!(delete_explanation.target_table, "accounts");
        assert_eq!(delete_explanation.selected_backend, "Native");
    }

    /// TD-110: VALUES DML remains on the native WAL/row-delta route even when
    /// the target is an OLAP-profile table with a preferred DataFusion route.
    /// DataFusion is for analytical reads/transforms and OLAP publication, not
    /// the direct authority for row-level PostgreSQL-style writes.
    #[tokio::test]
    async fn explain_values_insert_to_olap_table_stays_native() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE metrics (id TEXT NOT NULL, value FLOAT)
                 WITH (
                     workload = 'olap',
                     layout = 'columnar',
                     compute_route = 'datafusion-local',
                     freshness_sla = '10s'
                 );",
            )
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create olap table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(ExplainOnlyRecordStore),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        let stmt = parser
            .parse_dml("INSERT INTO metrics (id, value) VALUES ('m1', 42.0);")
            .expect("parse dml")
            .expect("dml stmt");

        let explanation = dml
            .explain_table_write(stmt)
            .await
            .expect("explain olap insert values");

        assert_eq!(explanation.target_table, "metrics");
        assert_eq!(
            explanation.selected_backend, "Native",
            "VALUES DML should commit through WAL/row-delta before OLAP publication"
        );
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
        assert!(
            explanation
                .rejected_paths
                .iter()
                .any(|path| path.backend == "DataFusionLocal"
                    && path.reason.contains("row/delta commit path")),
            "DataFusion rejection should explain the TD-110 row/delta gate: {:?}",
            explanation.rejected_paths
        );
        assert!(
            explanation.write_lane.contains("Wal"),
            "VALUES DML should remain WAL-backed, got {:?}",
            explanation.write_lane
        );
    }

    /// End-to-end smoke test: `DmlService` with a `DirectWalTableRecordStore` performs a
    /// VALUES INSERT and a primary-key SELECT through the canonical WAL + memtable path,
    /// then replays the WAL into a fresh memtable to verify durability.
    #[tokio::test]
    async fn direct_record_storage_insert_select_and_wal_replay() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

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
        let entries = replay_wal.read_entries().await.expect("read WAL entries");
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

    #[tokio::test]
    async fn insert_rejects_duplicate_primary_key() {
        // TD-110 Slice B: a second INSERT with the same primary key (and a duplicate within one
        // INSERT) is rejected; a distinct key still succeeds.
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

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
        let ddl_stmt = parser
            .parse_ddl("CREATE TABLE users (id TEXT NOT NULL, email TEXT NOT NULL, PRIMARY KEY (id));")
            .expect("parse create")
            .expect("ddl");
        ddl.execute(ddl_stmt).await.expect("create table");

        let wal_appender = Arc::new(
            FramedTableWalAppender::open(temp_dir.path().join("pk.wal"))
                .await
                .expect("open WAL"),
        );
        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            wal_appender,
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        let insert = |sql: &'static str| {
            let p = &parser;
            let stmt = p.parse_dml(sql).expect("parse dml").expect("dml");
            stmt
        };

        // First insert succeeds.
        dml.execute(insert(
            "INSERT INTO users (id, email) VALUES ('u1', 'a@x.com');",
        ))
        .await
        .expect("first insert");

        // Re-inserting the same PK is rejected against the committed row.
        let err = dml
            .execute(insert(
                "INSERT INTO users (id, email) VALUES ('u1', 'b@x.com');",
            ))
            .await
            .expect_err("duplicate PK must be rejected");
        assert!(
            err.to_string().contains("duplicate key value violates primary key"),
            "unexpected error: {err}"
        );

        // A duplicate PK within a single INSERT is also rejected.
        let err = dml
            .execute(insert(
                "INSERT INTO users (id, email) VALUES ('u2', 'c@x.com'), ('u2', 'd@x.com');",
            ))
            .await
            .expect_err("within-batch duplicate PK must be rejected");
        assert!(
            err.to_string().contains("appears more than once"),
            "unexpected error: {err}"
        );

        // A distinct key still inserts.
        dml.execute(insert(
            "INSERT INTO users (id, email) VALUES ('u3', 'e@x.com');",
        ))
        .await
        .expect("distinct key insert");
    }

    #[tokio::test]
    async fn insert_rejects_duplicate_unique_constraint() {
        // TD-110: a non-PK UNIQUE column rejects a duplicate value against a
        // committed row AND within one INSERT; NULL tuples are exempt (multiple
        // NULLs allowed); distinct values still insert.
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

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
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE members (id TEXT NOT NULL, email TEXT, PRIMARY KEY (id), UNIQUE (email));",
            )
            .expect("parse create")
            .expect("ddl");
        ddl.execute(ddl_stmt).await.expect("create table");

        let wal_appender = Arc::new(
            FramedTableWalAppender::open(temp_dir.path().join("unique.wal"))
                .await
                .expect("open WAL"),
        );
        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            wal_appender,
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        let insert = |sql: &'static str| {
            parser.parse_dml(sql).expect("parse dml").expect("dml")
        };

        // First insert succeeds.
        dml.execute(insert(
            "INSERT INTO members (id, email) VALUES ('m1', 'a@x.com');",
        ))
        .await
        .expect("first insert");

        // A different PK but duplicate UNIQUE email is rejected against the committed row.
        let err = dml
            .execute(insert(
                "INSERT INTO members (id, email) VALUES ('m2', 'a@x.com');",
            ))
            .await
            .expect_err("duplicate UNIQUE email must be rejected");
        assert!(
            err.to_string()
                .contains("duplicate key value violates unique constraint"),
            "unexpected error: {err}"
        );

        // A duplicate UNIQUE value within a single INSERT is also rejected.
        let err = dml
            .execute(insert(
                "INSERT INTO members (id, email) VALUES ('m3', 'b@x.com'), ('m4', 'b@x.com');",
            ))
            .await
            .expect_err("within-batch duplicate UNIQUE value must be rejected");
        assert!(
            err.to_string().contains("appears more than once"),
            "unexpected error: {err}"
        );

        // NULL UNIQUE tuples are exempt — multiple NULL emails are allowed.
        dml.execute(insert("INSERT INTO members (id) VALUES ('m5');"))
            .await
            .expect("first NULL email insert");
        dml.execute(insert("INSERT INTO members (id) VALUES ('m6');"))
            .await
            .expect("second NULL email allowed (NULLs exempt from UNIQUE)");

        // A distinct UNIQUE value still inserts.
        dml.execute(insert(
            "INSERT INTO members (id, email) VALUES ('m7', 'c@x.com');",
        ))
        .await
        .expect("distinct UNIQUE value insert");

        // Slice-C increment: a multi-row INSERT where ONE row (amid non-colliding
        // rows) duplicates a committed value is rejected by the single batch scan.
        let err = dml
            .execute(insert(
                "INSERT INTO members (id, email) VALUES ('m8', 'fresh1@x.com'), ('m9', 'c@x.com'), ('m10', 'fresh2@x.com');",
            ))
            .await
            .expect_err("a colliding row anywhere in the batch must be rejected");
        assert!(
            err.to_string()
                .contains("duplicate key value violates unique constraint"),
            "unexpected error: {err}"
        );

        // And a fully-distinct multi-row batch still inserts.
        dml.execute(insert(
            "INSERT INTO members (id, email) VALUES ('m11', 'd@x.com'), ('m12', 'e@x.com');",
        ))
        .await
        .expect("fully-distinct batch insert");
    }

    #[tokio::test]
    async fn unique_index_frees_value_on_delete_and_update() {
        // TD-110 Slice C: the store-layer UNIQUE index must release a value when
        // its owning row is DELETEd or UPDATEd off it, and re-claim it on the new
        // value — otherwise a duplicate would be wrongly rejected (stale index) or
        // wrongly accepted (missed update). Exercises the index maintenance path
        // (DirectWalTableRecordStore::check_unique_conflict override).
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

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
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE members (id TEXT NOT NULL, email TEXT, PRIMARY KEY (id), UNIQUE (email));",
            )
            .expect("parse create")
            .expect("ddl");
        ddl.execute(ddl_stmt).await.expect("create table");

        let wal_appender = Arc::new(
            FramedTableWalAppender::open(temp_dir.path().join("unique_idx.wal"))
                .await
                .expect("open WAL"),
        );
        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            wal_appender,
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        let run = |sql: &'static str| parser.parse_dml(sql).expect("parse dml").expect("dml");

        // DELETE frees the value: insert d1=x@x.com, delete it, then d2=x@x.com inserts.
        dml.execute(run("INSERT INTO members (id, email) VALUES ('d1', 'x@x.com');"))
            .await
            .expect("insert d1");
        dml.execute(run("DELETE FROM members WHERE id = 'd1';"))
            .await
            .expect("delete d1");
        dml.execute(run("INSERT INTO members (id, email) VALUES ('d2', 'x@x.com');"))
            .await
            .expect("x@x.com is free after delete — d2 must insert");

        // UPDATE moves a value: u3 holds y@x.com, update it to z@x.com.
        dml.execute(run("INSERT INTO members (id, email) VALUES ('u3', 'y@x.com');"))
            .await
            .expect("insert u3");
        dml.execute(run("UPDATE members SET email = 'z@x.com' WHERE id = 'u3';"))
            .await
            .expect("update u3 email");

        // The vacated value (y@x.com) is now insertable…
        dml.execute(run("INSERT INTO members (id, email) VALUES ('u4', 'y@x.com');"))
            .await
            .expect("y@x.com freed by update — u4 must insert");

        // …and the new value (z@x.com) is now claimed by u3 → rejected.
        let err = dml
            .execute(run("INSERT INTO members (id, email) VALUES ('u5', 'z@x.com');"))
            .await
            .expect_err("z@x.com taken by u3 after update — must be rejected");
        assert!(
            err.to_string()
                .contains("duplicate key value violates unique constraint"),
            "unexpected error: {err}"
        );
    }

    #[tokio::test]
    async fn update_rejects_duplicate_unique_value() {
        // TD-110: UPDATE that sets a UNIQUE column to a value owned by ANOTHER row
        // is rejected; setting it to the row's OWN current value (or a free value)
        // is allowed (the updated row is excluded from its own conflict check).
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

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
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE members (id TEXT NOT NULL, email TEXT, PRIMARY KEY (id), UNIQUE (email));",
            )
            .expect("parse create")
            .expect("ddl");
        ddl.execute(ddl_stmt).await.expect("create table");

        let wal_appender = Arc::new(
            FramedTableWalAppender::open(temp_dir.path().join("update_unique.wal"))
                .await
                .expect("open WAL"),
        );
        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            wal_appender,
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        let run = |sql: &'static str| parser.parse_dml(sql).expect("parse dml").expect("dml");

        dml.execute(run("INSERT INTO members (id, email) VALUES ('a', 'a@x.com');"))
            .await
            .expect("insert a");
        dml.execute(run("INSERT INTO members (id, email) VALUES ('b', 'b@x.com');"))
            .await
            .expect("insert b");

        // UPDATE a -> b@x.com (owned by b) must be rejected.
        let err = dml
            .execute(run("UPDATE members SET email = 'b@x.com' WHERE id = 'a';"))
            .await
            .expect_err("UPDATE to another row's unique value must be rejected");
        assert!(
            err.to_string()
                .contains("duplicate key value violates unique constraint"),
            "unexpected error: {err}"
        );

        // UPDATE a -> its OWN current value (a@x.com) is a no-op conflict-wise → allowed.
        dml.execute(run("UPDATE members SET email = 'a@x.com' WHERE id = 'a';"))
            .await
            .expect("UPDATE to the row's own current value must be allowed");

        // UPDATE a -> a free value is allowed.
        dml.execute(run("UPDATE members SET email = 'c@x.com' WHERE id = 'a';"))
            .await
            .expect("UPDATE to a free unique value must be allowed");
    }

    #[tokio::test]
    async fn insert_enforces_foreign_key_reference() {
        // TD-110: a FOREIGN KEY referencing the parent PK is enforced on INSERT —
        // present parent ok, missing parent rejected, NULL FK exempt; an UPDATE
        // re-checks. Parent + child live in the same store (same-partition).
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

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
        for create in [
            "CREATE TABLE customers (id TEXT NOT NULL, name TEXT, PRIMARY KEY (id));",
            "CREATE TABLE orders (id TEXT NOT NULL, customer_id TEXT, PRIMARY KEY (id), FOREIGN KEY (customer_id) REFERENCES customers (id));",
        ] {
            let stmt = parser.parse_ddl(create).expect("parse create").expect("ddl");
            ddl.execute(stmt).await.expect("create table");
        }

        let wal_appender = Arc::new(
            FramedTableWalAppender::open(temp_dir.path().join("fk.wal"))
                .await
                .expect("open WAL"),
        );
        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            wal_appender,
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        let run = |sql: &'static str| parser.parse_dml(sql).expect("parse dml").expect("dml");

        // Parent row, then a child referencing it — allowed.
        dml.execute(run("INSERT INTO customers (id, name) VALUES ('c1', 'Alice');"))
            .await
            .expect("insert parent customer");
        dml.execute(run("INSERT INTO orders (id, customer_id) VALUES ('o1', 'c1');"))
            .await
            .expect("child referencing existing parent must insert");

        // Child referencing a missing parent — rejected.
        let err = dml
            .execute(run("INSERT INTO orders (id, customer_id) VALUES ('o2', 'c99');"))
            .await
            .expect_err("FK to a missing parent must be rejected");
        assert!(
            err.to_string().contains("violates reference"),
            "unexpected error: {err}"
        );

        // NULL FK (customer_id omitted) is exempt.
        dml.execute(run("INSERT INTO orders (id) VALUES ('o3');"))
            .await
            .expect("NULL foreign key is exempt from the reference check");

        // UPDATE re-checks: pointing an order at a missing parent is rejected.
        let err = dml
            .execute(run("UPDATE orders SET customer_id = 'c99' WHERE id = 'o1';"))
            .await
            .expect_err("UPDATE to a missing FK parent must be rejected");
        assert!(
            err.to_string().contains("violates reference"),
            "unexpected error: {err}"
        );
    }

    /// SQL UPDATE and DELETE through `DirectWalTableRecordStore` — T9 conformance.
    ///
    /// Verifies that UPDATE rewrites the current visible record and DELETE leaves
    /// the row invisible to subsequent scans, both through the canonical WAL path.
    #[tokio::test]
    async fn direct_record_storage_update_and_delete_conformance() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

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
        assert_eq!(
            after_update.rows.len(),
            1,
            "SELECT must find i1 after update"
        );
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
            .select_table_records_with_projection("items", &["id".to_string()], None, &[])
            .await
            .expect("select after delete");
        let ids: Vec<&ProximaValue> = after_delete.rows.iter().map(|r| &r[0]).collect();
        assert!(
            !ids.contains(&&ProximaValue::String("i2".to_string())),
            "deleted row must not appear in scan"
        );
        assert_eq!(after_delete.rows.len(), 1, "only i1 must remain");
    }

    /// UPDATE/DELETE WHERE supports OR / nested groups / BETWEEN / NOT BETWEEN
    /// via the resolved predicate tree (reusing the catalog-aware leaf eval), and
    /// the PK fast-path stays OR-safe (a PK leaf under OR forces a full scan).
    #[tokio::test]
    async fn update_delete_support_or_nested_between_where() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("dml-tree.wal");
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
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE inv (id TEXT NOT NULL, status TEXT, qty INT, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl stmt");
        ddl.execute(ddl_stmt).await.expect("create table");

        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            Arc::new(
                FramedTableWalAppender::open(&wal_path)
                    .await
                    .expect("open WAL"),
            ),
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        async fn exec(
            dml: &DmlService,
            parser: &crate::query::sql_frontend::SqlFrontendParser,
            sql: &str,
        ) -> DmlResult {
            let stmt = parser.parse_dml(sql).expect("parse").expect("dml stmt");
            dml.execute(stmt).await.expect("execute")
        }
        async fn status_of(dml: &DmlService, id: &str) -> Option<String> {
            let sel = dml
                .select_table_records_with_projection(
                    "inv",
                    &["id".to_string(), "status".to_string()],
                    None,
                    &[RelationalSelectPredicateInput {
                        column_name: "id".to_string(),
                        condition: RelationalSelectPredicateCondition::Comparison {
                            operator: RelationalSelectPredicateOperator::Equal,
                            literal: id.to_string(),
                        },
                    }],
                )
                .await
                .expect("select");
            sel.rows.first().map(|row| match &row[1] {
                ProximaValue::String(s) => s.clone(),
                other => format!("{other:?}"),
            })
        }

        for (id, status, qty) in [
            ("i1", "active", 5),
            ("i2", "active", 15),
            ("i3", "idle", 25),
            ("i4", "idle", 35),
        ] {
            exec(
                &dml,
                &parser,
                &format!("INSERT INTO inv (id, status, qty) VALUES ('{id}', '{status}', {qty});"),
            )
            .await;
        }

        // (1) OR: status='active' OR qty >= 30 → i1,i2 (active) + i4 (qty 35).
        let r = exec(
            &dml,
            &parser,
            "UPDATE inv SET status = 'archived' WHERE status = 'active' OR qty >= 30;",
        )
        .await;
        assert_eq!(r.rows_affected, 3, "OR union");
        assert_eq!(status_of(&dml, "i1").await.as_deref(), Some("archived"));
        assert_eq!(status_of(&dml, "i2").await.as_deref(), Some("archived"));
        assert_eq!(
            status_of(&dml, "i3").await.as_deref(),
            Some("idle"),
            "untouched"
        );
        assert_eq!(status_of(&dml, "i4").await.as_deref(), Some("archived"));

        // (2) BETWEEN on an INT column (catalog-aware): qty 20..30 → i3 only.
        let r = exec(
            &dml,
            &parser,
            "UPDATE inv SET status = 'mid' WHERE qty BETWEEN 20 AND 30;",
        )
        .await;
        assert_eq!(r.rows_affected, 1, "BETWEEN matches i3 (qty 25)");
        assert_eq!(status_of(&dml, "i3").await.as_deref(), Some("mid"));

        // (3) NOT BETWEEN: qty outside 10..30 → i1 (5) and i4 (35).
        let r = exec(
            &dml,
            &parser,
            "UPDATE inv SET status = 'extreme' WHERE qty NOT BETWEEN 10 AND 30;",
        )
        .await;
        assert_eq!(r.rows_affected, 2, "NOT BETWEEN matches i1 + i4");
        assert_eq!(status_of(&dml, "i1").await.as_deref(), Some("extreme"));
        assert_eq!(status_of(&dml, "i4").await.as_deref(), Some("extreme"));

        // (4) Nested + PK-under-OR safety: id='i2' OR (status='extreme' AND qty < 10)
        // → i2 (PK) + i1 (extreme AND qty 5<10). The PK leaf under OR must NOT
        // shortcut to fetching only i2 and miss i1.
        let r = exec(
            &dml,
            &parser,
            "DELETE FROM inv WHERE id = 'i2' OR (status = 'extreme' AND qty < 10);",
        )
        .await;
        assert_eq!(
            r.rows_affected, 2,
            "i2 (pk) + i1 (nested) — PK fast-path stayed OR-safe"
        );
        assert_eq!(
            status_of(&dml, "i1").await,
            None,
            "i1 deleted via nested branch"
        );
        assert_eq!(
            status_of(&dml, "i2").await,
            None,
            "i2 deleted via PK branch"
        );
        assert_eq!(
            status_of(&dml, "i3").await.as_deref(),
            Some("mid"),
            "i3 survives"
        );
        assert_eq!(
            status_of(&dml, "i4").await.as_deref(),
            Some("extreme"),
            "i4 survives"
        );
    }

    /// SELECT WHERE supports OR / mixed-AND-OR / nested groups / NOT IN through
    /// the same resolved predicate tree as UPDATE/DELETE, pushed into the record
    /// scan via `select_table_records_with_projection_where`. The PK fast-path
    /// stays OR-safe (a PK leaf under OR forces a full scan), and nested groups
    /// are NOT flattened.
    #[tokio::test]
    async fn select_where_supports_or_nested_and_pk_or_safety() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("dml-select-tree.wal");
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
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE inv (id TEXT NOT NULL, status TEXT, qty INT, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl stmt");
        ddl.execute(ddl_stmt).await.expect("create table");

        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            Arc::new(
                FramedTableWalAppender::open(&wal_path)
                    .await
                    .expect("open WAL"),
            ),
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        for (id, status, qty) in [
            ("i1", "active", 5),
            ("i2", "active", 15),
            ("i3", "idle", 25),
            ("i4", "idle", 35),
        ] {
            let stmt = parser
                .parse_dml(&format!(
                    "INSERT INTO inv (id, status, qty) VALUES ('{id}', '{status}', {qty});"
                ))
                .expect("parse insert")
                .expect("insert stmt");
            dml.execute(stmt).await.expect("insert");
        }

        // Run a full SELECT string through the WhereClause-tree path; return the
        // chosen access path, the route-metadata predicate_count (tree leaf count),
        // and the sorted matching ids.
        async fn run(
            dml: &DmlService,
            parser: &crate::query::sql_frontend::SqlFrontendParser,
            sql: &str,
            limit: Option<usize>,
        ) -> (RelationalSelectAccessPath, usize, Vec<String>) {
            let where_clause = parser.parse_select_where_clause(sql).expect("parse where");
            let res = dml
                .select_table_records_with_projection_where(
                    "inv",
                    &["id".to_string()],
                    limit,
                    where_clause.as_ref(),
                )
                .await
                .expect("select");
            let mut ids: Vec<String> = res
                .rows
                .iter()
                .map(|row| match &row[0] {
                    ProximaValue::String(s) => s.clone(),
                    other => format!("{other:?}"),
                })
                .collect();
            ids.sort();
            (
                res.route_metadata.access_path,
                res.route_metadata.predicate_count,
                ids,
            )
        }

        // (1) OR union: status='active' OR qty >= 30 → i1,i2 (active) + i4 (35).
        // predicate_count = 2 leaves.
        let (path, pc, ids) = run(
            &dml,
            &parser,
            "SELECT id FROM inv WHERE status = 'active' OR qty >= 30",
            None,
        )
        .await;
        assert_eq!(path, RelationalSelectAccessPath::TableScan);
        assert_eq!(pc, 2, "route-metadata predicate_count == tree leaf count");
        assert_eq!(ids, vec!["i1", "i2", "i4"], "OR union");

        // (2) PK-under-OR safety: id='i2' OR status='idle'. The PK leaf must NOT
        // shortcut to a point lookup that misses the idle rows.
        let (path, _pc, ids) = run(
            &dml,
            &parser,
            "SELECT id FROM inv WHERE id = 'i2' OR status = 'idle'",
            None,
        )
        .await;
        assert_eq!(
            path,
            RelationalSelectAccessPath::TableScan,
            "PK leaf under OR must force a full scan"
        );
        assert_eq!(ids, vec!["i2", "i3", "i4"]);

        // (3) PK fast-path + full-predicate re-check: id IN (i1,i2,i3) AND
        // status='active' → only i1,i2 (i3 is idle and is dropped by the re-check).
        let (path, _pc, ids) = run(
            &dml,
            &parser,
            "SELECT id FROM inv WHERE id IN ('i1','i2','i3') AND status = 'active'",
            None,
        )
        .await;
        assert_eq!(path, RelationalSelectAccessPath::PrimaryKeyLookup);
        assert_eq!(ids, vec!["i1", "i2"]);

        // (4) Nested grouping must NOT flatten: status='idle' AND (qty < 30 OR
        // id='i1') → i3 only. Flattening to `idle AND qty<30 AND id='i1'` would
        // wrongly return zero rows. predicate_count = 3 leaves.
        let (path, pc, ids) = run(
            &dml,
            &parser,
            "SELECT id FROM inv WHERE status = 'idle' AND (qty < 30 OR id = 'i1')",
            None,
        )
        .await;
        assert_eq!(path, RelationalSelectAccessPath::TableScan);
        assert_eq!(pc, 3, "AND of [idle, OR(qty<30, id=i1)] has 3 leaves");
        assert_eq!(ids, vec!["i3"]);

        // (4b) OR-under-AND, no PK predicate → full scan, not flattened:
        // (status='active' OR qty >= 30) AND qty < 20 → {i1,i2,i4} ∩ {i1,i2} = i1,i2.
        let (path, _pc, ids) = run(
            &dml,
            &parser,
            "SELECT id FROM inv WHERE (status = 'active' OR qty >= 30) AND qty < 20",
            None,
        )
        .await;
        assert_eq!(path, RelationalSelectAccessPath::TableScan);
        assert_eq!(ids, vec!["i1", "i2"], "(a OR b) AND c grouping preserved");

        // (5) NOT IN mixed with OR over a never-true branch: qty NOT IN (5,15) OR
        // status IS NULL → i3,i4 (no row has a NULL status).
        let (_path, _pc, ids) = run(
            &dml,
            &parser,
            "SELECT id FROM inv WHERE qty NOT IN (5, 15) OR status IS NULL",
            None,
        )
        .await;
        assert_eq!(ids, vec!["i3", "i4"]);

        // (5b) NOT BETWEEN: qty outside 10..30 → i1 (5) and i4 (35).
        let (_path, _pc, ids) = run(
            &dml,
            &parser,
            "SELECT id FROM inv WHERE qty NOT BETWEEN 10 AND 30",
            None,
        )
        .await;
        assert_eq!(ids, vec!["i1", "i4"], "NOT BETWEEN matches the extremes");

        // (6) LIMIT honored on the OR scan path.
        let (_path, _pc, ids) = run(
            &dml,
            &parser,
            "SELECT id FROM inv WHERE status = 'active' OR status = 'idle'",
            Some(2),
        )
        .await;
        assert_eq!(ids.len(), 2, "limit pushed into the predicate scan");

        // (7) No WHERE → scan all rows.
        let (path, pc, ids) = run(&dml, &parser, "SELECT id FROM inv", None).await;
        assert_eq!(path, RelationalSelectAccessPath::TableScan);
        assert_eq!(pc, 0, "no WHERE → zero predicate leaves");
        assert_eq!(ids, vec!["i1", "i2", "i3", "i4"]);
    }

    /// `scan_table_relational` (PATH B reader backend) pushes the output
    /// projection + a full-row predicate + limit into the record-store scan.
    #[tokio::test]
    async fn scan_table_relational_pushes_projection_predicate_limit() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("dml-scan-rel.wal");
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
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE inv (id TEXT NOT NULL, status TEXT, qty INT, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl stmt");
        ddl.execute(ddl_stmt).await.expect("create table");

        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            Arc::new(
                FramedTableWalAppender::open(&wal_path)
                    .await
                    .expect("open WAL"),
            ),
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        for (id, status, qty) in [
            ("i1", "active", 5),
            ("i2", "active", 15),
            ("i3", "idle", 25),
            ("i4", "idle", 35),
        ] {
            let stmt = parser
                .parse_dml(&format!(
                    "INSERT INTO inv (id, status, qty) VALUES ('{id}', '{status}', {qty});"
                ))
                .expect("parse insert")
                .expect("insert stmt");
            dml.execute(stmt).await.expect("insert");
        }

        // (a) No predicate / no projection → all rows, full column order [id,status,qty].
        let (schema, rows) = dml
            .scan_table_relational("inv", None, None, None)
            .await
            .expect("scan all");
        assert_eq!(schema.columns.len(), 3);
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|r| r.len() == 3));

        // (b) Predicate over the FULL row: status (ordinal 1) == 'active' → i1,i2.
        let pred =
            |row: &[ProximaValue]| matches!(&row[1], ProximaValue::String(s) if s == "active");
        let (_s, rows) = dml
            .scan_table_relational("inv", None, Some(&pred), None)
            .await
            .expect("scan predicate");
        let mut ids: Vec<String> = rows
            .iter()
            .map(|r| match &r[0] {
                ProximaValue::String(s) => s.clone(),
                other => format!("{other:?}"),
            })
            .collect();
        ids.sort();
        assert_eq!(ids, vec!["i1", "i2"], "predicate filters to active rows");

        // (c) Output projection narrows + orders columns → just [status].
        let cols = vec!["status".to_string()];
        let (_s, rows) = dml
            .scan_table_relational("inv", Some(&cols), None, None)
            .await
            .expect("scan projection");
        assert_eq!(rows.len(), 4);
        assert!(rows.iter().all(|r| r.len() == 1));

        // (d) Limit caps the result.
        let (_s, rows) = dml
            .scan_table_relational("inv", None, None, Some(2))
            .await
            .expect("scan limit");
        assert_eq!(rows.len(), 2, "limit caps the scan");
    }

    /// P3.2: `materialize_table_to_parquet` snapshots the table's rows to a Parquet
    /// object on the bridge AND flips the catalog layout to Parquet/ProjectionPublication
    /// at the published location, so the OLAP router will treat it as Parquet-backed.
    #[tokio::test]
    async fn materialize_table_writes_parquet_and_flips_catalog_layout() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};
        use futures::StreamExt;
        use proximadb_iceberg_engine::IcebergObjectStoreBridge;

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("dml-materialize.wal");
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
        let ddl_stmt = parser
            .parse_ddl("CREATE TABLE inv (id TEXT NOT NULL, status TEXT, qty INT, PRIMARY KEY (id));")
            .expect("parse ddl")
            .expect("ddl stmt");
        ddl.execute(ddl_stmt).await.expect("create table");

        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            Arc::new(FramedTableWalAppender::open(&wal_path).await.expect("open WAL")),
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        for (id, status, qty) in [("i1", "active", 5), ("i2", "active", 15), ("i3", "idle", 25)] {
            let stmt = parser
                .parse_dml(&format!(
                    "INSERT INTO inv (id, status, qty) VALUES ('{id}', '{status}', {qty});"
                ))
                .expect("parse insert")
                .expect("insert stmt");
            dml.execute(stmt).await.expect("insert");
        }

        // A shared in-memory bridge: we reuse the SAME handle to read the snapshot
        // back (from_url("memory://") would open a fresh, empty store).
        let bridge = Arc::new(IcebergObjectStoreBridge::from_url("memory:///warehouse").unwrap());

        let location = dml
            .materialize_table_to_parquet(&*bridge, "memory:///warehouse", "inv", None)
            .await
            .expect("materialize");

        // The published location is the tenant-isolated base URL.
        assert_eq!(location, "memory:///warehouse/data/default_tenant/default/inv");

        // The Parquet snapshot landed where the OLAP reader lists `{location}/data/*.parquet`,
        // and reads back all three rows.
        let data_object = object_store::path::Path::from(
            "data/default_tenant/default/inv/data/part-0.parquet",
        );
        let mut stream = bridge
            .read_parquet_batches(&data_object, Arc::new(arrow_schema::Schema::empty()), 1024, None)
            .await
            .expect("read materialized parquet");
        let mut total = 0usize;
        while let Some(batch) = stream.next().await {
            total += batch.expect("batch").num_rows();
        }
        assert_eq!(total, 3, "all rows materialized into the snapshot");

        // The catalog layout is now a published Parquet projection at the location.
        let (catalog, id) = manager.resolve_table("inv").await.expect("resolve");
        let schema = catalog.get_table(&id).await.expect("get table");
        assert_eq!(schema.storage_layouts.len(), 1);
        let layout = &schema.storage_layouts[0];
        assert!(matches!(
            layout.physical_format,
            proximadb_catalog::CatalogPhysicalFormat::Parquet
        ));
        assert!(matches!(
            layout.authority,
            proximadb_catalog::CatalogAuthorityMode::ProjectionPublication
        ));
        assert_eq!(layout.location.as_deref(), Some(location.as_str()));
    }

    /// P3 end-to-end: materialize a table to a Parquet snapshot on a REOPENABLE
    /// (file://) object store, then read it back through the DataFusion OLAP reader
    /// (`ObjectStoreParquetTable::open(location)` + `ctx.sql`) — proving the published
    /// `location` is exactly what the router registers and queries. Feature-gated
    /// because the DataFusion reader lives behind `datafusion-integration`.
    #[cfg(feature = "datafusion-integration")]
    #[tokio::test]
    async fn materialized_table_is_readable_through_datafusion_reader() {
        use crate::datafusion::create_session_context;
        use crate::datafusion::engine_adapters::register_object_store_parquet_location;
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};
        use proximadb_iceberg_engine::IcebergObjectStoreBridge;

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("dml-mat-e2e.wal");
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
        let ddl_stmt = parser
            .parse_ddl("CREATE TABLE inv (id TEXT NOT NULL, status TEXT, qty INT, PRIMARY KEY (id));")
            .expect("parse ddl")
            .expect("ddl stmt");
        ddl.execute(ddl_stmt).await.expect("create table");
        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            Arc::new(FramedTableWalAppender::open(&wal_path).await.expect("open WAL")),
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        for (id, status, qty) in [("i1", "active", 5), ("i2", "active", 15), ("i3", "idle", 25)] {
            let stmt = parser
                .parse_dml(&format!(
                    "INSERT INTO inv (id, status, qty) VALUES ('{id}', '{status}', {qty});"
                ))
                .expect("parse insert")
                .expect("insert stmt");
            dml.execute(stmt).await.expect("insert");
        }

        // A file:// store the OLAP reader can REOPEN from the published URL.
        let store_dir = tempfile::tempdir().expect("store tempdir");
        let root_url = format!("file://{}", store_dir.path().display());
        let bridge = Arc::new(IcebergObjectStoreBridge::from_url(&root_url).expect("bridge"));

        let location = dml
            .materialize_table_to_parquet(&*bridge, &root_url, "inv", None)
            .await
            .expect("materialize");

        // Read the published location back through the DataFusion OLAP reader.
        let ctx = create_session_context().expect("session ctx");
        register_object_store_parquet_location(&ctx, "inv_parquet", &location)
            .await
            .expect("register parquet location");
        let batches = ctx
            .sql("SELECT * FROM inv_parquet")
            .await
            .expect("plan select")
            .collect()
            .await
            .expect("collect");
        let total: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(total, 3, "DataFusion reads all materialized rows from the published location");
    }

    /// `point_lookup_relational` (PATH B PkLookup backend) returns the full row by
    /// primary key in `schema.columns` order, and `None` for a missing key.
    #[tokio::test]
    async fn point_lookup_relational_returns_full_row_by_pk() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("dml-pklookup.wal");
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
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE inv (id TEXT NOT NULL, status TEXT, qty INT, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl stmt");
        ddl.execute(ddl_stmt).await.expect("create table");

        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            Arc::new(
                FramedTableWalAppender::open(&wal_path)
                    .await
                    .expect("open WAL"),
            ),
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        for (id, status, qty) in [("i1", "active", 5), ("i2", "active", 15)] {
            let stmt = parser
                .parse_dml(&format!(
                    "INSERT INTO inv (id, status, qty) VALUES ('{id}', '{status}', {qty});"
                ))
                .expect("parse insert")
                .expect("insert stmt");
            dml.execute(stmt).await.expect("insert");
        }

        // Existing key → full row [id, status, qty] in schema order.
        let row = dml
            .point_lookup_relational("inv", "i2")
            .await
            .expect("lookup")
            .expect("row present");
        assert_eq!(row.len(), 3);
        assert_eq!(row[0], ProximaValue::String("i2".to_string()));
        assert_eq!(row[1], ProximaValue::String("active".to_string()));
        assert_eq!(row[2], ProximaValue::Int32(15));

        // Missing key → None.
        let missing = dml
            .point_lookup_relational("inv", "nope")
            .await
            .expect("lookup");
        assert!(missing.is_none(), "absent key returns None");
    }

    /// UPDATE/DELETE WHERE must honor NON-primary-key predicates (and the full
    /// predicate of a mixed `pk = x AND col = y`), via the shared scan-filter
    /// push-down — not the prior PK-only `extract_ids_from_where` which silently
    /// ignored non-PK conditions.
    #[tokio::test]
    async fn update_delete_honor_non_primary_key_where_predicates() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("dml-nonpk.wal");
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
        let ddl_stmt = parser
            .parse_ddl("CREATE TABLE orders (id TEXT NOT NULL, status TEXT, PRIMARY KEY (id));")
            .expect("parse ddl")
            .expect("ddl stmt");
        ddl.execute(ddl_stmt).await.expect("create table");

        let wal_appender = Arc::new(
            FramedTableWalAppender::open(&wal_path)
                .await
                .expect("open WAL"),
        );
        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            wal_appender,
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        async fn exec(
            dml: &DmlService,
            parser: &crate::query::sql_frontend::SqlFrontendParser,
            sql: &str,
        ) -> DmlResult {
            let stmt = parser.parse_dml(sql).expect("parse").expect("dml stmt");
            dml.execute(stmt).await.expect("execute")
        }
        async fn status_of(dml: &DmlService, id: &str) -> Option<String> {
            let sel = dml
                .select_table_records_with_projection(
                    "orders",
                    &["id".to_string(), "status".to_string()],
                    None,
                    &[RelationalSelectPredicateInput {
                        column_name: "id".to_string(),
                        condition: RelationalSelectPredicateCondition::Comparison {
                            operator: RelationalSelectPredicateOperator::Equal,
                            literal: id.to_string(),
                        },
                    }],
                )
                .await
                .expect("select");
            sel.rows.first().map(|row| match &row[1] {
                ProximaValue::String(s) => s.clone(),
                other => format!("{other:?}"),
            })
        }

        for (id, status) in [("o1", "active"), ("o2", "active"), ("o3", "inactive")] {
            exec(
                &dml,
                &parser,
                &format!("INSERT INTO orders (id, status) VALUES ('{id}', '{status}');"),
            )
            .await;
        }

        // (1) Non-PK WHERE UPDATE: only the two 'active' rows change.
        let r = exec(
            &dml,
            &parser,
            "UPDATE orders SET status = 'archived' WHERE status = 'active';",
        )
        .await;
        assert!(r.success);
        assert_eq!(r.rows_affected, 2, "only the two active rows update");
        assert_eq!(status_of(&dml, "o1").await.as_deref(), Some("archived"));
        assert_eq!(status_of(&dml, "o2").await.as_deref(), Some("archived"));
        assert_eq!(
            status_of(&dml, "o3").await.as_deref(),
            Some("inactive"),
            "non-matching row must be untouched"
        );

        // (2) Mixed pk + non-pk, the silent-bug fix: o3 is 'inactive', so the
        // `AND status = 'active'` must prevent the update even though id matches.
        let r = exec(
            &dml,
            &parser,
            "UPDATE orders SET status = 'hacked' WHERE id = 'o3' AND status = 'active';",
        )
        .await;
        assert!(r.success);
        assert_eq!(
            r.rows_affected, 0,
            "id matches but the non-PK condition fails"
        );
        assert_eq!(
            status_of(&dml, "o3").await.as_deref(),
            Some("inactive"),
            "row must NOT be mutated when the full predicate is not satisfied"
        );

        // (3) Non-PK WHERE DELETE: removes only the archived rows.
        let r = exec(
            &dml,
            &parser,
            "DELETE FROM orders WHERE status = 'archived';",
        )
        .await;
        assert!(r.success);
        assert_eq!(r.rows_affected, 2);
        assert_eq!(status_of(&dml, "o1").await, None, "o1 deleted");
        assert_eq!(status_of(&dml, "o2").await, None, "o2 deleted");
        assert_eq!(
            status_of(&dml, "o3").await.as_deref(),
            Some("inactive"),
            "o3 survives"
        );
    }

    /// INSERT and DELETE through `DmlService` bump catalog row-count statistics
    /// so that subsequent route decisions reflect the current approximate cardinality.
    #[tokio::test]
    async fn insert_and_delete_update_catalog_row_count_statistics() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("stats-feedback.wal");

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
        let create_sql = "CREATE TABLE stat_rows (id TEXT NOT NULL, val TEXT, PRIMARY KEY (id));";
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
        let record_store = Arc::new(DirectWalTableRecordStore::new(
            Arc::new(MemtableRecordStorage::new()),
            wal_appender,
        ));
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            record_store,
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        // Pre-condition: row_count starts at 0 (no stats written yet).
        let (catalog_pre, table_id_pre) = manager
            .resolve_table("stat_rows")
            .await
            .expect("resolve table");
        let stats_pre = catalog_pre
            .get_statistics(&table_id_pre)
            .await
            .unwrap_or_default();
        assert_eq!(stats_pre.row_count, 0, "row_count must start at 0");

        // INSERT 3 rows → bump_row_count_stats adds +3.
        for i in 1..=3u32 {
            let sql = format!(
                "INSERT INTO stat_rows (id, val) VALUES ('r{}', 'v{}');",
                i, i
            );
            let stmt = parser
                .parse_dml(&sql)
                .expect("parse insert")
                .expect("dml stmt");
            dml.execute(stmt).await.expect("insert");
        }

        let (catalog_after_insert, table_id_after_insert) = manager
            .resolve_table("stat_rows")
            .await
            .expect("resolve table after insert");
        let stats_after_insert = catalog_after_insert
            .get_statistics(&table_id_after_insert)
            .await
            .unwrap_or_default();
        assert_eq!(
            stats_after_insert.row_count, 3,
            "row_count must be 3 after three inserts"
        );

        // DELETE 1 row → bump_row_count_stats subtracts 1.
        let del_stmt = parser
            .parse_dml("DELETE FROM stat_rows WHERE id = 'r1';")
            .expect("parse delete")
            .expect("dml stmt");
        dml.execute(del_stmt).await.expect("delete");

        let (catalog_after_delete, table_id_after_delete) = manager
            .resolve_table("stat_rows")
            .await
            .expect("resolve table after delete");
        let stats_after_delete = catalog_after_delete
            .get_statistics(&table_id_after_delete)
            .await
            .unwrap_or_default();
        assert_eq!(
            stats_after_delete.row_count, 2,
            "row_count must be 2 after one delete"
        );
        assert!(
            stats_after_delete.last_analyzed_ms.is_some(),
            "last_analyzed_ms must be set"
        );
    }

    /// T8: After INSERT with some NULL column values, `column_stats[col].null_count`
    /// reflects the number of NULLs written. Null-free inserts leave null_count at 0/absent.
    #[tokio::test]
    async fn insert_null_values_update_column_null_count_statistics() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("col-stats.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl("CREATE TABLE nullable_tbl (id TEXT NOT NULL, note TEXT, score FLOAT);")
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        // Row 1: note = NULL, score present.
        // Row 2: note present, score = NULL.
        // Row 3: both present.
        for stmt_sql in [
            "INSERT INTO nullable_tbl (id, note, score) VALUES ('r1', NULL, 1.0);",
            "INSERT INTO nullable_tbl (id, note, score) VALUES ('r2', 'hello', NULL);",
            "INSERT INTO nullable_tbl (id, note, score) VALUES ('r3', 'world', 2.0);",
        ] {
            let stmt = parser
                .parse_dml(stmt_sql)
                .expect("parse insert")
                .expect("dml stmt");
            dml.execute(stmt).await.expect("insert");
        }

        let (catalog, table_id) = manager
            .resolve_table("nullable_tbl")
            .await
            .expect("resolve");
        let stats = catalog.get_statistics(&table_id).await.unwrap_or_default();

        assert_eq!(stats.row_count, 3, "three rows inserted");
        assert_eq!(
            stats.column_stats.get("note").and_then(|cs| cs.null_count),
            Some(1),
            "note has 1 NULL across 3 inserts"
        );
        assert_eq!(
            stats.column_stats.get("score").and_then(|cs| cs.null_count),
            Some(1),
            "score has 1 NULL across 3 inserts"
        );
        // id is NOT NULL — null_count entry should be absent or 0.
        let id_null_count = stats
            .column_stats
            .get("id")
            .and_then(|cs| cs.null_count)
            .unwrap_or(0);
        assert_eq!(
            id_null_count, 0,
            "id is NOT NULL, no nulls should be counted"
        );
    }

    /// T9: After INSERT with NULL in nullable columns, `scan_table_records` returns rows with
    /// `ProximaValue::Null` for those fields, and projection produces empty string for NULL values.
    #[tokio::test]
    async fn insert_nullable_values_are_scannable_and_project_null_correctly() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("scan-null.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl("CREATE TABLE scan_null_tbl (id TEXT NOT NULL, tag TEXT, rating FLOAT, PRIMARY KEY (id));")
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        for sql in [
            "INSERT INTO scan_null_tbl (id, tag, rating) VALUES ('x1', NULL, 9.5);",
            "INSERT INTO scan_null_tbl (id, tag, rating) VALUES ('x2', 'beta', NULL);",
        ] {
            let stmt = parser.parse_dml(sql).expect("parse").expect("dml");
            dml.execute(stmt).await.expect("insert");
        }

        let (_schema, records) = dml
            .scan_table_records("scan_null_tbl", None)
            .await
            .expect("scan");
        assert_eq!(records.len(), 2, "two rows scanned");

        let find = |oid: &str| {
            records
                .iter()
                .find(|r| r.oid == oid)
                .unwrap_or_else(|| panic!("row {oid} not found"))
        };
        let prop_value = |record: &ProximaRecord, col: &str| -> Option<ProximaValue> {
            match record.props.get(col) {
                Some(proximadb_records::ProximaTreeNode::Value(v)) => Some(v.clone()),
                _ => None,
            }
        };
        let r_x1 = find("x1");
        assert_eq!(
            prop_value(r_x1, "tag"),
            Some(ProximaValue::Null),
            "x1.tag should be Null"
        );
        let r_x2 = find("x2");
        assert_eq!(
            prop_value(r_x2, "rating"),
            Some(ProximaValue::Null),
            "x2.rating should be Null"
        );

        // Projection: NULL columns surface as ProximaValue::Null in SELECT output.
        let result = dml
            .select_table_records_with_projection(
                "scan_null_tbl",
                &["id".to_string(), "tag".to_string(), "rating".to_string()],
                None,
                &[],
            )
            .await
            .expect("select");
        let x1_id = ProximaValue::String("x1".to_string());
        let x2_id = ProximaValue::String("x2".to_string());
        let row_x1 = result
            .rows
            .iter()
            .find(|r| r.first() == Some(&x1_id))
            .expect("x1 row in projection");
        // columns order: id, tag, rating → indices 0, 1, 2
        assert_eq!(
            row_x1.get(1),
            Some(&ProximaValue::Null),
            "x1.tag projects as Null"
        );
        let row_x2 = result
            .rows
            .iter()
            .find(|r| r.first() == Some(&x2_id))
            .expect("x2 row in projection");
        assert_eq!(
            row_x2.get(2),
            Some(&ProximaValue::Null),
            "x2.rating projects as Null"
        );
    }

    /// T9: `IS NULL` and `IS NOT NULL` predicates correctly filter rows with `ProximaValue::Null`
    /// versus non-null values in `scan_table_records_with_predicates`.
    #[tokio::test]
    async fn is_null_predicate_filters_nullable_column_rows() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("predicate-null.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE null_pred_tbl (id TEXT NOT NULL, label TEXT, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        for sql in [
            "INSERT INTO null_pred_tbl (id, label) VALUES ('p1', NULL);",
            "INSERT INTO null_pred_tbl (id, label) VALUES ('p2', 'hello');",
            "INSERT INTO null_pred_tbl (id, label) VALUES ('p3', NULL);",
        ] {
            let stmt = parser.parse_dml(sql).expect("parse").expect("dml");
            dml.execute(stmt).await.expect("insert");
        }

        // IS NULL: should return p1 and p3 only.
        let is_null_predicate = RelationalSelectPredicateInput {
            column_name: "label".to_string(),
            condition: RelationalSelectPredicateCondition::IsNull { negated: false },
        };
        let (_schema, null_rows) = dml
            .scan_table_records_with_predicates("null_pred_tbl", None, &[is_null_predicate])
            .await
            .expect("scan IS NULL");
        let null_oids: Vec<&str> = null_rows.iter().map(|r| r.oid.as_str()).collect();
        assert!(null_oids.contains(&"p1"), "p1 must match IS NULL");
        assert!(null_oids.contains(&"p3"), "p3 must match IS NULL");
        assert!(!null_oids.contains(&"p2"), "p2 must not match IS NULL");

        // IS NOT NULL: should return only p2.
        let is_not_null_predicate = RelationalSelectPredicateInput {
            column_name: "label".to_string(),
            condition: RelationalSelectPredicateCondition::IsNull { negated: true },
        };
        let (_schema, not_null_rows) = dml
            .scan_table_records_with_predicates("null_pred_tbl", None, &[is_not_null_predicate])
            .await
            .expect("scan IS NOT NULL");
        let not_null_oids: Vec<&str> = not_null_rows.iter().map(|r| r.oid.as_str()).collect();
        assert!(not_null_oids.contains(&"p2"), "p2 must match IS NOT NULL");
        assert!(
            !not_null_oids.contains(&"p1"),
            "p1 must not match IS NOT NULL"
        );
        assert!(
            !not_null_oids.contains(&"p3"),
            "p3 must not match IS NOT NULL"
        );
    }

    /// T4: NOT IN predicate via `scan_table_records_with_predicates` excludes rows whose
    /// column value appears in the exclusion set; IN includes only rows whose value is in the set.
    #[tokio::test]
    async fn in_and_not_in_predicates_filter_correctly() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("in-pred.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl("CREATE TABLE in_pred_tbl (id TEXT NOT NULL, status TEXT NOT NULL, PRIMARY KEY (id));")
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        for sql in [
            "INSERT INTO in_pred_tbl (id, status) VALUES ('i1', 'active');",
            "INSERT INTO in_pred_tbl (id, status) VALUES ('i2', 'inactive');",
            "INSERT INTO in_pred_tbl (id, status) VALUES ('i3', 'pending');",
            "INSERT INTO in_pred_tbl (id, status) VALUES ('i4', 'active');",
        ] {
            let stmt = parser.parse_dml(sql).expect("parse").expect("dml");
            dml.execute(stmt).await.expect("insert");
        }

        // IN ('active', 'pending'): should return i1, i3, i4.
        let in_predicate = RelationalSelectPredicateInput {
            column_name: "status".to_string(),
            condition: RelationalSelectPredicateCondition::In {
                literals: vec!["active".to_string(), "pending".to_string()],
                negated: false,
            },
        };
        let (_schema, in_rows) = dml
            .scan_table_records_with_predicates("in_pred_tbl", None, &[in_predicate])
            .await
            .expect("scan IN");
        let in_oids: Vec<&str> = in_rows.iter().map(|r| r.oid.as_str()).collect();
        assert!(in_oids.contains(&"i1"), "i1 (active) matches IN");
        assert!(in_oids.contains(&"i3"), "i3 (pending) matches IN");
        assert!(in_oids.contains(&"i4"), "i4 (active) matches IN");
        assert!(!in_oids.contains(&"i2"), "i2 (inactive) excluded from IN");

        // NOT IN ('active'): should return i2 and i3.
        let not_in_predicate = RelationalSelectPredicateInput {
            column_name: "status".to_string(),
            condition: RelationalSelectPredicateCondition::In {
                literals: vec!["active".to_string()],
                negated: true,
            },
        };
        let (_schema, not_in_rows) = dml
            .scan_table_records_with_predicates("in_pred_tbl", None, &[not_in_predicate])
            .await
            .expect("scan NOT IN");
        let not_in_oids: Vec<&str> = not_in_rows.iter().map(|r| r.oid.as_str()).collect();
        assert!(
            not_in_oids.contains(&"i2"),
            "i2 (inactive) matches NOT IN ('active')"
        );
        assert!(
            not_in_oids.contains(&"i3"),
            "i3 (pending) matches NOT IN ('active')"
        );
        assert!(
            !not_in_oids.contains(&"i1"),
            "i1 (active) excluded by NOT IN"
        );
        assert!(
            !not_in_oids.contains(&"i4"),
            "i4 (active) excluded by NOT IN"
        );
    }

    /// T4: LIKE and NOT LIKE predicates filter rows correctly via `scan_table_records_with_predicates`.
    #[tokio::test]
    async fn like_predicate_filters_correctly() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("like-pred.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE like_tbl (id TEXT NOT NULL, name TEXT NOT NULL, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        for sql in [
            "INSERT INTO like_tbl (id, name) VALUES ('l1', 'alice_admin');",
            "INSERT INTO like_tbl (id, name) VALUES ('l2', 'bob_user');",
            "INSERT INTO like_tbl (id, name) VALUES ('l3', 'alice_user');",
            "INSERT INTO like_tbl (id, name) VALUES ('l4', 'charlie');",
        ] {
            let stmt = parser.parse_dml(sql).expect("parse").expect("dml");
            dml.execute(stmt).await.expect("insert");
        }

        // LIKE 'alice%': should return l1 and l3.
        let like_predicate = RelationalSelectPredicateInput {
            column_name: "name".to_string(),
            condition: RelationalSelectPredicateCondition::Like {
                pattern: "alice%".to_string(),
                negated: false,
            },
        };
        let (_schema, like_rows) = dml
            .scan_table_records_with_predicates("like_tbl", None, &[like_predicate])
            .await
            .expect("scan LIKE");
        let like_oids: Vec<&str> = like_rows.iter().map(|r| r.oid.as_str()).collect();
        assert!(
            like_oids.contains(&"l1"),
            "l1 (alice_admin) matches LIKE 'alice%'"
        );
        assert!(
            like_oids.contains(&"l3"),
            "l3 (alice_user) matches LIKE 'alice%'"
        );
        assert!(!like_oids.contains(&"l2"), "l2 (bob_user) excluded");
        assert!(!like_oids.contains(&"l4"), "l4 (charlie) excluded");

        // NOT LIKE 'alice%': should return l2 and l4.
        let not_like_predicate = RelationalSelectPredicateInput {
            column_name: "name".to_string(),
            condition: RelationalSelectPredicateCondition::Like {
                pattern: "alice%".to_string(),
                negated: true,
            },
        };
        let (_schema, not_like_rows) = dml
            .scan_table_records_with_predicates("like_tbl", None, &[not_like_predicate])
            .await
            .expect("scan NOT LIKE");
        let not_like_oids: Vec<&str> = not_like_rows.iter().map(|r| r.oid.as_str()).collect();
        assert!(
            not_like_oids.contains(&"l2"),
            "l2 (bob_user) matches NOT LIKE 'alice%'"
        );
        assert!(
            not_like_oids.contains(&"l4"),
            "l4 (charlie) matches NOT LIKE 'alice%'"
        );
        assert!(!not_like_oids.contains(&"l1"), "l1 excluded by NOT LIKE");
        assert!(!not_like_oids.contains(&"l3"), "l3 excluded by NOT LIKE");
    }

    /// T9: UPDATE SET col = NULL on a nullable column succeeds and the column reads back as
    /// `ProximaValue::Null`; UPDATE SET col = NULL on a NOT NULL column is rejected.
    #[tokio::test]
    async fn update_nullable_column_to_null_succeeds() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("update-null.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl("CREATE TABLE upd_null_tbl (id TEXT NOT NULL, note TEXT, score FLOAT NOT NULL, PRIMARY KEY (id));")
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        let insert_stmt = parser
            .parse_dml("INSERT INTO upd_null_tbl (id, note, score) VALUES ('u1', 'initial', 1.5);")
            .expect("parse")
            .expect("dml");
        dml.execute(insert_stmt).await.expect("insert");

        // UPDATE nullable column to NULL — should succeed.
        let upd_stmt = parser
            .parse_dml("UPDATE upd_null_tbl SET note = NULL WHERE id = 'u1';")
            .expect("parse update")
            .expect("dml");
        dml.execute(upd_stmt)
            .await
            .expect("UPDATE note=NULL should succeed for nullable column");

        let prop_val = |record: &ProximaRecord, col: &str| -> Option<ProximaValue> {
            match record.props.get(col) {
                Some(proximadb_records::ProximaTreeNode::Value(v)) => Some(v.clone()),
                _ => None,
            }
        };

        let (_schema, rows) = dml
            .scan_table_records("upd_null_tbl", None)
            .await
            .expect("scan");
        let u1 = rows.iter().find(|r| r.oid == "u1").expect("u1 row");
        assert_eq!(
            prop_val(u1, "note"),
            Some(ProximaValue::Null),
            "note should be Null after UPDATE SET note=NULL"
        );

        // UPDATE NOT NULL column to NULL — should be rejected.
        let bad_upd_stmt = parser
            .parse_dml("UPDATE upd_null_tbl SET score = NULL WHERE id = 'u1';")
            .expect("parse bad update")
            .expect("dml");
        let err = dml.execute(bad_upd_stmt).await;
        assert!(
            err.is_err(),
            "UPDATE score=NULL should fail for NOT NULL column"
        );
        let err_msg = err.unwrap_err().to_string();
        assert!(
            err_msg.contains("cannot be NULL") || err_msg.contains("not nullable"),
            "error should mention NULL constraint: {err_msg}"
        );
    }

    /// CREATE TABLE via `DdlService` appears in `information_schema.tables` and `columns`,
    /// and `DmlService` can resolve the table metadata immediately after DDL. Covers T9
    /// DDL metadata round-trip: catalog write → introspection read → DML resolve.
    #[tokio::test]
    async fn ddl_create_table_visible_in_introspection_and_resolvable_by_dml() {
        use crate::services::CatalogIntrospectionService;

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
        let create_sql = "CREATE TABLE meta_test (id TEXT NOT NULL, label TEXT, score DECIMAL(10,4), PRIMARY KEY (id));";
        let ddl_stmt = parser
            .parse_ddl(create_sql)
            .expect("parse create table")
            .expect("ddl stmt");
        ddl.execute(ddl_stmt).await.expect("create table");

        // information_schema.tables must include the newly created table.
        let introspection = CatalogIntrospectionService::new(manager.clone());
        let result = introspection
            .execute_select(
                "SELECT table_schema, table_name FROM information_schema.tables WHERE table_name = 'meta_test'",
            )
            .await
            .expect("catalog introspection query")
            .expect("must return a result");
        let tables_result = result
            .rows
            .iter()
            .any(|row| row.iter().any(|v| v.contains("meta_test")));
        assert!(
            tables_result,
            "meta_test must appear in information_schema.tables"
        );

        // information_schema.columns must include all declared columns.
        let col_result = introspection
            .execute_select(
                "SELECT column_name FROM information_schema.columns WHERE table_name = 'meta_test'",
            )
            .await
            .expect("columns introspection query")
            .expect("must return columns result");
        let all_values: Vec<&str> = col_result
            .rows
            .iter()
            .flat_map(|row| row.iter().map(|v| v.as_str()))
            .collect();
        assert!(
            all_values.contains(&"id"),
            "id column must appear in information_schema.columns"
        );
        assert!(
            all_values.contains(&"label"),
            "label column must appear in information_schema.columns"
        );
        assert!(
            all_values.contains(&"score"),
            "score column must appear in information_schema.columns"
        );

        // DmlService must be able to resolve the table — verifies catalog → DML integration.
        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(ExplainOnlyRecordStore),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        let (catalog, table_id) = manager
            .resolve_table("meta_test")
            .await
            .expect("DmlService must resolve DDL-created table");
        let schema = catalog
            .get_table(&table_id)
            .await
            .expect("get table schema");
        assert_eq!(schema.name, "meta_test");
        assert_eq!(schema.primary_key.len(), 1);
        assert_eq!(schema.primary_key[0], "id");

        // Explain a write plan into the table — end-to-end DDL → route planner round-trip.
        let explain_stmt = parser
            .parse_dml("INSERT INTO meta_test SELECT * FROM meta_test;")
            .expect("parse explain dml")
            .expect("dml stmt");
        let explanation = dml
            .explain_table_write(explain_stmt)
            .await
            .expect("explain table write must succeed for DDL-created table");
        assert_eq!(explanation.target_table, "meta_test");
    }

    #[tokio::test]
    async fn ddl_constraints_surface_as_route_metadata_gaps() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let customers = parser
            .parse_ddl("CREATE TABLE customers (id TEXT NOT NULL, PRIMARY KEY (id));")
            .expect("parse customers")
            .expect("customers ddl");
        DdlService::new(manager.clone())
            .execute(customers)
            .await
            .expect("create customers");

        let orders = parser
            .parse_ddl(
                "CREATE TABLE orders_with_constraints (
                    id TEXT NOT NULL,
                    email TEXT,
                    customer_id TEXT,
                    amount FLOAT,
                    PRIMARY KEY (id),
                    UNIQUE (email),
                    CHECK (amount > 0),
                    FOREIGN KEY (customer_id) REFERENCES customers(id)
                );",
            )
            .expect("parse orders")
            .expect("orders ddl");
        DdlService::new(manager.clone())
            .execute(orders)
            .await
            .expect("create orders");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(ExplainOnlyRecordStore),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );
        let explain_stmt = parser
            .parse_dml("INSERT INTO orders_with_constraints SELECT * FROM orders_with_constraints;")
            .expect("parse explain dml")
            .expect("dml stmt");
        let explanation = dml
            .explain_table_write(explain_stmt)
            .await
            .expect("explain table write");

        assert_eq!(explanation.target_table, "orders_with_constraints");
        assert!(
            explanation
                .route_metadata
                .constraint_enforcement
                .starts_with("partial_native_enforced:")
        );
        assert!(
            explanation
                .route_metadata
                .constraint_gaps
                .contains(&"unique_indexes_cataloged_not_enforced".to_string())
        );
        assert!(
            explanation
                .route_metadata
                .constraint_enforcement
                .contains("check")
        );
        assert!(
            explanation
                .route_metadata
                .constraint_enforcement
                .contains("unique_non_null_fail_closed")
        );
        assert!(
            explanation
                .route_metadata
                .constraint_enforcement
                .contains("foreign_key_non_null_fail_closed")
        );
        assert!(
            explanation
                .route_metadata
                .constraint_gaps
                .contains(&"foreign_keys_cataloged_not_enforced".to_string())
        );
    }

    #[tokio::test]
    async fn dml_enforces_foreign_key_rejecting_missing_parent() {
        let temp_dir = tempfile::tempdir().expect("tempdir");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        for ddl in [
            "CREATE TABLE customers_for_fk (id TEXT NOT NULL, PRIMARY KEY (id));",
            "CREATE TABLE orders_with_fk (id TEXT NOT NULL, customer_id TEXT, PRIMARY KEY (id), FOREIGN KEY (customer_id) REFERENCES customers_for_fk(id));",
        ] {
            let stmt = parser.parse_ddl(ddl).expect("parse ddl").expect("ddl stmt");
            DdlService::new(manager.clone())
                .execute(stmt)
                .await
                .expect("create table");
        }

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager,
            Arc::new(ExplainOnlyRecordStore),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        // TD-110: FK references are now enforced (DmlService::enforce_foreign_keys).
        // No parent row exists (the explain-only store has no records), so the
        // child insert is rejected as a reference violation rather than the old
        // "not enforced yet" fail-close.
        let fk_insert = parser
            .parse_dml("INSERT INTO orders_with_fk (id, customer_id) VALUES ('o1', 'c1');")
            .expect("parse fk insert")
            .expect("fk insert");
        let fk_err = dml
            .execute(fk_insert)
            .await
            .expect_err("FK to a non-existent parent must be rejected");
        assert!(
            fk_err.to_string().contains("violates reference"),
            "unexpected error: {fk_err}"
        );
    }

    /// T15: `validate_record_batch_against_schema` passes for conforming records and returns `Err`
    /// when a NOT NULL column receives `ProximaValue::Null` in a fast-lane (non-SQL) batch write.
    #[tokio::test]
    async fn fast_lane_schema_validation_rejects_null_for_not_null_column() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("fast-lane-schema.wal");

        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE fast_lane_tbl (id TEXT NOT NULL, label TEXT, score FLOAT NOT NULL, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        // Conforming record: id=text, score=float, label=null (nullable).
        let ok_record = ProximaRecord {
            oid: "v1".to_string(),
            props: proximadb_records::ProximaTree::from([
                (
                    "id".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String("v1".to_string())),
                ),
                (
                    "score".to_string(),
                    ProximaTreeNode::Value(ProximaValue::Float32(3.14)),
                ),
                (
                    "label".to_string(),
                    ProximaTreeNode::Value(ProximaValue::Null),
                ),
            ]),
            ..Default::default()
        };
        dml.validate_record_batch_against_schema("fast_lane_tbl", &[ok_record])
            .await
            .expect("conforming record must pass schema validation");

        // Violating record: score is NOT NULL but receives Null.
        let bad_record = ProximaRecord {
            oid: "v2".to_string(),
            props: proximadb_records::ProximaTree::from([
                (
                    "id".to_string(),
                    ProximaTreeNode::Value(ProximaValue::String("v2".to_string())),
                ),
                (
                    "score".to_string(),
                    ProximaTreeNode::Value(ProximaValue::Null),
                ),
            ]),
            ..Default::default()
        };
        let err = dml
            .validate_record_batch_against_schema("fast_lane_tbl", &[bad_record])
            .await;
        assert!(
            err.is_err(),
            "NOT NULL column with Null must fail fast-lane validation"
        );
        let err_val = err.unwrap_err();
        let chain: String = err_val
            .chain()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(": ");
        assert!(
            chain.contains("not nullable") || chain.contains("cannot be NULL"),
            "error chain should name the constraint: {chain}"
        );
    }

    /// T15: `validate_record_batch_against_schema` silently passes when the collection is not
    /// registered as a relational table (non-relational / vector-only collections stay open).
    #[tokio::test]
    async fn fast_lane_schema_validation_skips_non_relational_collections() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("fast-lane-skip.wal");

        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        // "unknown_collection" is not in xCatalog — validation must silently pass.
        let any_record = ProximaRecord {
            oid: "x1".to_string(),
            props: proximadb_records::ProximaTree::from([(
                "score".to_string(),
                ProximaTreeNode::Value(ProximaValue::Null),
            )]),
            ..Default::default()
        };
        dml.validate_record_batch_against_schema("unknown_collection", &[any_record])
            .await
            .expect("non-relational collection must skip schema validation");
    }

    /// T11: `explain_analyze_table_write` executes the write and returns the route explanation
    /// enriched with `execution_elapsed_us` and `execution_rows_written`.
    #[tokio::test]
    async fn explain_analyze_executes_write_and_returns_execution_stats() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("explain-analyze.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl("CREATE TABLE analyze_tbl (id TEXT NOT NULL, val INTEGER NOT NULL, PRIMARY KEY (id));")
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        let stmt = parser
            .parse_dml("INSERT INTO analyze_tbl (id, val) VALUES ('a1', 42), ('a2', 99);")
            .expect("parse")
            .expect("dml");
        let explanation = dml
            .explain_analyze_table_write(stmt)
            .await
            .expect("explain analyze must succeed");

        assert_eq!(explanation.target_table, "analyze_tbl");
        assert!(
            explanation.execution_elapsed_us.is_some(),
            "elapsed_us must be populated by EXPLAIN ANALYZE"
        );
        assert_eq!(
            explanation.execution_rows_written,
            Some(2),
            "rows_written must reflect the 2 inserted rows"
        );
    }

    /// T8: After INSERT, `column_stats[col].min_value` and `max_value` are updated to the
    /// lexicographic min/max of the inserted values for String and integer columns.
    #[tokio::test]
    async fn insert_updates_column_min_max_statistics() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("minmax-stats.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE minmax_tbl (id TEXT NOT NULL, name TEXT NOT NULL, score INTEGER NOT NULL, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        for sql in [
            "INSERT INTO minmax_tbl (id, name, score) VALUES ('r1', 'charlie', 30);",
            "INSERT INTO minmax_tbl (id, name, score) VALUES ('r2', 'alice', 10);",
            "INSERT INTO minmax_tbl (id, name, score) VALUES ('r3', 'bob', 20);",
        ] {
            let stmt = parser.parse_dml(sql).expect("parse").expect("dml");
            dml.execute(stmt).await.expect("insert");
        }

        let (catalog, table_id) = manager.resolve_table("minmax_tbl").await.expect("resolve");
        let stats = catalog.get_statistics(&table_id).await.unwrap_or_default();

        // name is a TEXT column: min = 'alice', max = 'charlie'
        let name_stats = stats.column_stats.get("name").expect("name col stats");
        assert_eq!(
            name_stats.min_value.as_deref(),
            Some("alice"),
            "name min should be 'alice'"
        );
        assert_eq!(
            name_stats.max_value.as_deref(),
            Some("charlie"),
            "name max should be 'charlie'"
        );

        // score is an INTEGER column: min = +000000000000000000010, max = +000000000000000000030
        let score_stats = stats.column_stats.get("score").expect("score col stats");
        assert!(
            score_stats.min_value.is_some(),
            "score min must be populated"
        );
        assert!(
            score_stats.max_value.is_some(),
            "score max must be populated"
        );
        // The sortable min/max string for integer 10 sorts before 30.
        assert!(
            score_stats.min_value < score_stats.max_value,
            "score min must sort before max: min={:?} max={:?}",
            score_stats.min_value,
            score_stats.max_value
        );
    }

    #[tokio::test]
    async fn insert_updates_column_ndv_statistics() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("ndv-stats.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE ndv_tbl (id TEXT NOT NULL, category TEXT NOT NULL, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        // Insert 4 rows: 3 distinct categories ('a', 'b', 'c'), 4 distinct ids.
        for sql in [
            "INSERT INTO ndv_tbl (id, category) VALUES ('r1', 'a');",
            "INSERT INTO ndv_tbl (id, category) VALUES ('r2', 'b');",
            "INSERT INTO ndv_tbl (id, category) VALUES ('r3', 'c');",
            "INSERT INTO ndv_tbl (id, category) VALUES ('r4', 'a');",
        ] {
            let stmt = parser.parse_dml(sql).expect("parse").expect("dml");
            dml.execute(stmt).await.expect("insert");
        }

        let (catalog, table_id) = manager.resolve_table("ndv_tbl").await.expect("resolve");
        let stats = catalog.get_statistics(&table_id).await.unwrap_or_default();

        // Verify row count (sanity check for the cap logic)
        assert_eq!(stats.row_count, 4, "row count must be 4");

        // id has 4 distinct values across the 4 single-row batches → additive estimate = 4
        let id_ndv = stats
            .column_stats
            .get("id")
            .and_then(|s| s.distinct_count)
            .expect("id distinct_count must be populated");
        assert!(id_ndv >= 1, "id NDV must be at least 1, got {id_ndv}");
        assert!(
            id_ndv <= stats.row_count,
            "id NDV ({id_ndv}) must not exceed row count ({})",
            stats.row_count
        );

        // category has values within each single-row batch → additive estimate = 4 (one per batch),
        // capped at row_count = 4.
        let cat_ndv = stats
            .column_stats
            .get("category")
            .and_then(|s| s.distinct_count)
            .expect("category distinct_count must be populated");
        assert!(
            cat_ndv >= 1,
            "category NDV must be at least 1, got {cat_ndv}"
        );
        assert!(
            cat_ndv <= stats.row_count,
            "category NDV ({cat_ndv}) must not exceed row count ({})",
            stats.row_count
        );
    }

    /// T18: cross-surface conformance — DML SQL INSERT and fast-lane
    /// `validate_record_batch_against_schema` must make the same accept/reject decision
    /// for the same logical row, so REST/gRPC/Arrow Flight callers see the same constraint
    /// behavior as SQL clients.
    #[tokio::test]
    async fn dml_and_fast_lane_agree_on_not_null_constraint() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("conformance.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE conf_tbl (id TEXT NOT NULL, label TEXT NOT NULL, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        // -------- conforming row: both surfaces must accept --------
        let dml_ok = parser
            .parse_dml("INSERT INTO conf_tbl (id, label) VALUES ('k1', 'present');")
            .expect("parse")
            .expect("dml");
        dml.execute(dml_ok)
            .await
            .expect("DML must accept conforming row");

        let mut ok_props = std::collections::HashMap::new();
        ok_props.insert(
            "id".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("k2".to_string())),
        );
        ok_props.insert(
            "label".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("present".to_string())),
        );
        let ok_record = ProximaRecord {
            oid: "k2".to_string(),
            props: ok_props,
            ..Default::default()
        };
        dml.validate_record_batch_against_schema("conf_tbl", &[ok_record])
            .await
            .expect("fast-lane must accept conforming row");

        // -------- violating row: both surfaces must reject --------
        let dml_bad = parser
            .parse_dml("INSERT INTO conf_tbl (id, label) VALUES ('k3', NULL);")
            .expect("parse")
            .expect("dml");
        let dml_err = dml
            .execute(dml_bad)
            .await
            .expect_err("DML must reject NULL for NOT NULL column");

        let mut bad_props = std::collections::HashMap::new();
        bad_props.insert(
            "id".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("k4".to_string())),
        );
        bad_props.insert(
            "label".to_string(),
            ProximaTreeNode::Value(ProximaValue::Null),
        );
        let bad_record = ProximaRecord {
            oid: "k4".to_string(),
            props: bad_props,
            ..Default::default()
        };
        let fast_lane_err = dml
            .validate_record_batch_against_schema("conf_tbl", &[bad_record])
            .await
            .expect_err("fast-lane must reject NULL for NOT NULL column");

        // Both error chains must reference the constraint that was violated.
        let dml_chain: String = dml_err
            .chain()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(": ");
        let fast_lane_chain: String = fast_lane_err
            .chain()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(": ");
        let mentions_constraint = |s: &str| {
            s.contains("not nullable") || s.contains("cannot be NULL") || s.contains("NOT NULL")
        };
        assert!(
            mentions_constraint(&dml_chain),
            "DML error chain should explain NOT NULL violation: {dml_chain}"
        );
        assert!(
            mentions_constraint(&fast_lane_chain),
            "fast-lane error chain should explain NOT NULL violation: {fast_lane_chain}"
        );
    }

    /// T18: cross-surface conformance — DML SQL INSERT and fast-lane validation must agree on
    /// type mismatches. A string value in an integer column must be rejected by both surfaces
    /// regardless of the exact error wording.
    #[tokio::test]
    async fn dml_and_fast_lane_agree_on_type_mismatch() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("type-conformance.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE type_tbl (id TEXT NOT NULL, score INTEGER NOT NULL, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        // DML SQL path: 'not-an-int' in an INTEGER column must fail at literal coercion.
        let dml_bad = parser
            .parse_dml("INSERT INTO type_tbl (id, score) VALUES ('k1', 'not-an-int');")
            .expect("parse")
            .expect("dml");
        let dml_err = dml
            .execute(dml_bad)
            .await
            .expect_err("DML must reject string literal for INTEGER column");

        // Fast-lane path: ProximaValue::String in an Int32 column must fail validation.
        let mut bad_props = std::collections::HashMap::new();
        bad_props.insert(
            "id".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("k2".to_string())),
        );
        bad_props.insert(
            "score".to_string(),
            ProximaTreeNode::Value(ProximaValue::String("not-an-int".to_string())),
        );
        let bad_record = ProximaRecord {
            oid: "k2".to_string(),
            props: bad_props,
            ..Default::default()
        };
        let fast_lane_err = dml
            .validate_record_batch_against_schema("type_tbl", &[bad_record])
            .await
            .expect_err("fast-lane must reject ProximaValue::String for INTEGER column");

        // Both error chains must mention the integer column or expected type so callers can
        // diagnose the violation. Exact wording differs between paths; both must be informative.
        let dml_chain: String = dml_err
            .chain()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(": ");
        let fast_lane_chain: String = fast_lane_err
            .chain()
            .map(|e| e.to_string())
            .collect::<Vec<_>>()
            .join(": ");
        let mentions_type = |s: &str| {
            let lower = s.to_lowercase();
            lower.contains("integer") || lower.contains("int32") || lower.contains("int64")
        };
        assert!(
            mentions_type(&dml_chain),
            "DML error chain should explain integer-type violation: {dml_chain}"
        );
        assert!(
            mentions_type(&fast_lane_chain),
            "fast-lane error chain should explain integer-type violation: {fast_lane_chain}"
        );
    }

    #[tokio::test]
    async fn insert_marks_statistics_with_last_analyzed_timestamp() {
        use crate::services::record_store::DirectWalTableRecordStore;
        use crate::services::{FramedTableWalAppender, MemtableRecordStorage};

        let temp_dir = tempfile::tempdir().expect("tempdir");
        let wal_path = temp_dir.path().join("stale-stats.wal");
        let manager = Arc::new(CatalogManager::new());
        manager
            .create_native_catalog("native", temp_dir.path().to_string_lossy().as_ref())
            .await
            .expect("native catalog");
        DdlService::new(manager.clone())
            .execute(DdlStatement::CreateNamespace {
                namespace: vec!["default".to_string()],
                if_not_exists: true,
                properties: HashMap::new(),
            })
            .await
            .expect("namespace");

        let parser = crate::query::sql_frontend::SqlFrontendParser::new();
        let ddl_stmt = parser
            .parse_ddl(
                "CREATE TABLE stale_tbl (id TEXT NOT NULL, val INTEGER NOT NULL, PRIMARY KEY (id));",
            )
            .expect("parse ddl")
            .expect("ddl");
        DdlService::new(manager.clone())
            .execute(ddl_stmt)
            .await
            .expect("create table");

        let dml = DmlService::with_record_store_and_table_write_executor(
            manager.clone(),
            Arc::new(DirectWalTableRecordStore::new(
                Arc::new(MemtableRecordStorage::new()),
                Arc::new(
                    FramedTableWalAppender::open(&wal_path)
                        .await
                        .expect("open WAL"),
                ),
            )),
            Arc::new(PlannedOnlyTableWriteExecutor::new()),
        );

        let pre_insert_ms = DmlService::now_unix_ms();
        let stmt = parser
            .parse_dml("INSERT INTO stale_tbl (id, val) VALUES ('r1', 42);")
            .expect("parse")
            .expect("dml");
        dml.execute(stmt).await.expect("insert");
        let post_insert_ms = DmlService::now_unix_ms();

        let (catalog, table_id) = manager.resolve_table("stale_tbl").await.expect("resolve");
        let stats = catalog.get_statistics(&table_id).await.unwrap_or_default();

        // last_analyzed_ms must be set and within the wall-clock window of the INSERT.
        let last_ms = stats
            .last_analyzed_ms
            .expect("last_analyzed_ms must be populated after INSERT");
        assert!(
            last_ms >= pre_insert_ms && last_ms <= post_insert_ms,
            "last_analyzed_ms ({last_ms}) must be within [{pre_insert_ms}, {post_insert_ms}]"
        );

        // Stats are fresh inside a generous TTL window and stale outside it.
        assert!(
            !stats.is_stale(last_ms, 60_000),
            "stats updated at now must not be stale within a 60s TTL"
        );
        assert!(
            stats.is_stale(last_ms + 120_000, 60_000),
            "stats 120s old must be stale under a 60s TTL"
        );
    }

    #[test]
    fn sql_like_fast_paths_match_dp() {
        // Each case is (value, pattern). The fast path and DP must agree.
        let cases: &[(&str, &str)] = &[
            // exact-match fast path
            ("hello", "hello"),
            ("hello", "world"),
            ("", ""),
            // suffix fast path: leading %
            ("hello world", "%world"),
            ("hello world", "%earth"),
            ("", "%"),
            // prefix fast path: trailing %
            ("hello world", "hello%"),
            ("hello world", "world%"),
            // contains fast path: %x%
            ("hello world", "%lo wo%"),
            ("hello world", "%missing%"),
            // single % in middle: DP path
            ("hello world", "he%ld"),
            ("hello world", "ab%cd"),
            // underscore: DP path
            ("abc", "a_c"),
            ("abc", "a_d"),
            ("abc", "a__"),
            // mixed wildcards: DP path
            ("hello world", "h_llo%world"),
            // empty pattern, non-empty value
            ("abc", ""),
            // pattern with multiple percents only (matches everything)
            ("abc", "%%"),
            ("", "%%"),
            // multi-byte (non-ASCII) — must take DP path
            ("héllo", "héllo"),
            ("héllo", "h%o"),
            ("héllo", "h_llo"),
        ];

        for (value, pattern) in cases {
            let fast = DmlService::sql_like_matches(value, pattern);
            let dp = DmlService::sql_like_matches_dp(value, pattern);
            assert_eq!(
                fast, dp,
                "LIKE mismatch for value={value:?} pattern={pattern:?}: fast={fast} dp={dp}"
            );
        }
    }
}
