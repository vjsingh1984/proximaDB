//! Routed table-write execution contract.
//!
//! `table_write_plan` decides which backend/access method should execute a
//! table-to-table write. This module owns the next boundary: taking a routed
//! plan and executing it through native record writers, DataFusion, or an
//! external open-table commit protocol. The first implementation is deliberately
//! planned-only so pgwire/DML can depend on a stable executor trait before the
//! concrete readers and writers are wired in.

use std::{fmt, sync::Arc};

use anyhow::{Result, anyhow};
use async_trait::async_trait;
use proximadb_catalog::CatalogTableSchema;
use proximadb_records::ProximaRecord;

use crate::query::table_write_plan::{
    ComputeBackend, ExecutionGuard, ReadSource, RoutedExecutionPlan, WriteMode,
};
use crate::services::WriteLane;
use crate::services::record_store::{
    TableRecordMutation, TableRecordMutationKind, TableRecordScanRequest, TableRecordStore,
};

/// Execution status for a routed table-write plan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableWriteExecutionStatus {
    /// The plan was validated and routed, but no physical execution happened.
    PlannedOnly,
    /// The plan completed and committed through the selected writer.
    Completed,
}

/// Request passed to the table-write executor.
#[derive(Debug, Clone)]
pub struct TableWriteExecutionRequest<'a> {
    /// Resolved xCatalog target schema.
    pub target_schema: &'a CatalogTableSchema,
    /// Resolved xCatalog source schema when the source is another catalog table.
    pub source_schema: Option<&'a CatalogTableSchema>,
    /// Routed plan selected by `DmlWritePlanner` / `TableWriteRouter`.
    pub routed_plan: RoutedExecutionPlan,
}

/// Result returned by a table-write executor implementation.
#[derive(Debug, Clone)]
pub struct TableWriteExecutionResult {
    /// Whether execution only reached the planned boundary or actually committed.
    pub status: TableWriteExecutionStatus,
    /// Number of rows committed. Planned-only executions report zero.
    pub rows_written: u64,
    /// Human-readable backend/access-method summary.
    pub route_summary: String,
    /// Guards validated or still required before physical execution.
    pub guards: Vec<ExecutionGuard>,
}

impl TableWriteExecutionResult {
    pub fn planned(routed_plan: &RoutedExecutionPlan) -> Self {
        Self {
            status: TableWriteExecutionStatus::PlannedOnly,
            rows_written: 0,
            route_summary: format!(
                "backend={:?}, access_method={:?}",
                routed_plan.backend, routed_plan.selected_path.access_method
            ),
            guards: routed_plan.required_guards.clone(),
        }
    }
}

/// Execution boundary for `INSERT ... SELECT`, `INSERT OVERWRITE`, CTAS, and MERGE.
#[async_trait]
pub trait TableWriteExecutor: Send + Sync {
    /// Execute a routed table-write plan.
    async fn execute(
        &self,
        request: TableWriteExecutionRequest<'_>,
    ) -> Result<TableWriteExecutionResult>;
}

/// Structured source-reader failure class.
///
/// Keep the class name in user-facing errors until protocol error payloads can
/// carry structured codes end to end.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TableRecordSourceErrorClass {
    RetryableReadFailure,
    PermanentReadFailure,
    PolicyViolation,
    TypeViolation,
    StaleSnapshot,
}

impl fmt::Display for TableRecordSourceErrorClass {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}

/// Error returned by source readers before data becomes canonical records.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TableRecordSourceError {
    pub class: TableRecordSourceErrorClass,
    pub message: String,
}

impl TableRecordSourceError {
    pub fn new(class: TableRecordSourceErrorClass, message: impl Into<String>) -> Self {
        Self {
            class,
            message: message.into(),
        }
    }
}

impl fmt::Display for TableRecordSourceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.class, self.message)
    }
}

impl std::error::Error for TableRecordSourceError {}

/// Canonical source reader used by native table-write execution.
///
/// DataFusion, native table scans, Arrow Flight, and external file readers can
/// all adapt into this trait by yielding catalog-validated `ProximaRecord`
/// batches. The executor owns write semantics; source adapters only read.
#[async_trait]
pub trait TableRecordSourceReader: Send + Sync {
    /// Read the next source batch. `Ok(None)` means end-of-stream.
    async fn next_batch(
        &self,
        source: &ReadSource,
        source_schema: Option<&CatalogTableSchema>,
        target_schema: &CatalogTableSchema,
        cursor: &mut TableRecordSourceCursor,
    ) -> Result<Option<Vec<ProximaRecord>>>;
}

/// Per-execution source cursor.
///
/// Source readers are shared services, so executor-local cursor state prevents
/// one `INSERT ... SELECT` execution from consuming state belonging to another.
#[derive(Debug, Default)]
pub struct TableRecordSourceCursor {
    buffered_records: Option<Vec<ProximaRecord>>,
    offset: usize,
}

impl TableRecordSourceCursor {
    fn take_next(&mut self, batch_size: usize) -> Option<Vec<ProximaRecord>> {
        let records = self.buffered_records.as_ref()?;
        if self.offset >= records.len() {
            return None;
        }

        let end = self.offset.saturating_add(batch_size).min(records.len());
        let batch = records[self.offset..end].to_vec();
        self.offset = end;
        Some(batch)
    }
}

/// Native source reader backed by the canonical `TableRecordStore` scan method.
///
/// This reader supports catalog-table sources. General `SELECT` queries with
/// projection/filter/join/aggregate semantics should route through DataFusion
/// or another compute adapter and yield canonical record batches from there.
pub struct TableRecordStoreSourceReader {
    record_store: Arc<dyn TableRecordStore>,
    batch_size: usize,
}

impl TableRecordStoreSourceReader {
    pub fn new(record_store: Arc<dyn TableRecordStore>) -> Self {
        Self::with_batch_size(record_store, 1024)
    }

    pub fn with_batch_size(record_store: Arc<dyn TableRecordStore>, batch_size: usize) -> Self {
        Self {
            record_store,
            batch_size: batch_size.max(1),
        }
    }
}

#[async_trait]
impl TableRecordSourceReader for TableRecordStoreSourceReader {
    async fn next_batch(
        &self,
        source: &ReadSource,
        source_schema: Option<&CatalogTableSchema>,
        target_schema: &CatalogTableSchema,
        cursor: &mut TableRecordSourceCursor,
    ) -> Result<Option<Vec<ProximaRecord>>> {
        let ReadSource::CatalogTable { table, .. } = source else {
            return Err(TableRecordSourceError::new(
                TableRecordSourceErrorClass::PermanentReadFailure,
                format!(
                    "Native table-write source reader supports catalog-table sources only; source {:?} requires a compute adapter",
                    source
                ),
            )
            .into());
        };
        let scan_schema = source_schema.unwrap_or(target_schema);
        if scan_schema.name != table.name {
            return Err(TableRecordSourceError::new(
                TableRecordSourceErrorClass::TypeViolation,
                format!(
                    "Catalog-table source '{}' does not match resolved schema '{}'",
                    table.qualified_name(),
                    scan_schema.name
                ),
            )
            .into());
        }

        if cursor.buffered_records.is_none() {
            let records = self
                .record_store
                .scan_records(
                    scan_schema,
                    TableRecordScanRequest {
                        table_id: table.qualified_name(),
                        limit: None,
                        include_vector: true,
                        include_props: true,
                    },
                    None,
                )
                .await?;
            cursor.buffered_records = Some(records);
        }

        Ok(cursor.take_next(self.batch_size))
    }
}

/// Native executor that commits canonical source batches through `TableRecordStore`.
pub struct NativeTableWriteExecutor {
    source_reader: Arc<dyn TableRecordSourceReader>,
    record_store: Arc<dyn TableRecordStore>,
}

impl NativeTableWriteExecutor {
    pub fn new(
        source_reader: Arc<dyn TableRecordSourceReader>,
        record_store: Arc<dyn TableRecordStore>,
    ) -> Self {
        Self {
            source_reader,
            record_store,
        }
    }
}

#[async_trait]
impl TableWriteExecutor for NativeTableWriteExecutor {
    async fn execute(
        &self,
        request: TableWriteExecutionRequest<'_>,
    ) -> Result<TableWriteExecutionResult> {
        validate_required_guards(request.target_schema, &request.routed_plan)?;
        if request.routed_plan.backend != ComputeBackend::Native {
            return Ok(TableWriteExecutionResult::planned(&request.routed_plan));
        }
        validate_native_write_lane(request.target_schema, &request.routed_plan)?;

        let mutation_kind = mutation_kind_for_write_mode(&request.routed_plan.plan.write_mode)?;
        let mut rows_written = 0;
        let mut cursor = TableRecordSourceCursor::default();

        while let Some(batch) = self
            .source_reader
            .next_batch(
                &request.routed_plan.plan.source,
                request.source_schema,
                request.target_schema,
                &mut cursor,
            )
            .await?
        {
            if batch.is_empty() {
                continue;
            }
            let mutations = batch
                .into_iter()
                .map(|record| TableRecordMutation::new(mutation_kind, record))
                .collect::<Vec<_>>();
            let result = self
                .record_store
                .write_mutations(request.target_schema, mutations, None)
                .await?;
            if !result.success {
                return Err(anyhow!(
                    "Native table write failed for '{}': {:?}",
                    request.target_schema.name,
                    result.errors
                ));
            }
            rows_written += result.record_ids.len() as u64;
        }

        Ok(TableWriteExecutionResult {
            status: TableWriteExecutionStatus::Completed,
            rows_written,
            route_summary: format!(
                "backend={:?}, access_method={:?}",
                request.routed_plan.backend, request.routed_plan.selected_path.access_method
            ),
            guards: request.routed_plan.required_guards,
        })
    }
}

/// Planned-only executor used until concrete DataFusion/native/open-table paths exist.
#[derive(Debug, Default)]
pub struct PlannedOnlyTableWriteExecutor;

impl PlannedOnlyTableWriteExecutor {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl TableWriteExecutor for PlannedOnlyTableWriteExecutor {
    async fn execute(
        &self,
        request: TableWriteExecutionRequest<'_>,
    ) -> Result<TableWriteExecutionResult> {
        validate_required_guards(request.target_schema, &request.routed_plan)?;
        Ok(TableWriteExecutionResult::planned(&request.routed_plan))
    }
}

fn mutation_kind_for_write_mode(write_mode: &WriteMode) -> Result<TableRecordMutationKind> {
    match write_mode {
        WriteMode::Append | WriteMode::InsertOnly => Ok(TableRecordMutationKind::Insert),
        WriteMode::Upsert => Ok(TableRecordMutationKind::Upsert),
        WriteMode::OverwriteTable => Ok(TableRecordMutationKind::OverwriteSnapshot),
        WriteMode::ReplacePartitions(_) => Ok(TableRecordMutationKind::ReplacePartitions),
        WriteMode::Merge => Err(anyhow!(
            "MERGE table-write execution requires merge predicate support"
        )),
    }
}

fn validate_required_guards(
    target_schema: &CatalogTableSchema,
    routed_plan: &RoutedExecutionPlan,
) -> Result<()> {
    let guards = &routed_plan.required_guards;
    for required in [
        ExecutionGuard::PinSourceSnapshot,
        ExecutionGuard::CheckTargetWriteCapabilities,
    ] {
        if !guards.contains(&required) {
            return Err(anyhow!(
                "Routed table-write plan for '{}' is missing required guard {:?}",
                target_schema.name,
                required
            ));
        }
    }
    Ok(())
}

fn validate_native_write_lane(
    target_schema: &CatalogTableSchema,
    routed_plan: &RoutedExecutionPlan,
) -> Result<()> {
    if routed_plan.write_lane_decision.lane == WriteLane::WalCurrentState {
        return Ok(());
    }

    Err(anyhow!(
        "Native table-write executor for '{}' can commit {:?} only after the dedicated {:?} commit protocol is implemented",
        target_schema.name,
        routed_plan.plan.write_mode,
        routed_plan.write_lane_decision.lane
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::table_write_plan::{
        CopyIntoPlan, CostEstimate, LogicalTableRef, TableWriteRouter, WriteIntentOverrides,
        WriteMode,
    };
    use crate::services::operations::OperationMetrics;
    use crate::services::record_store::{
        TableRecordGetRequest, TableRecordGetResponse, TableRecordScanRequest,
        TableRecordWriteResult,
    };
    use crate::services::{DEFAULT_BULK_BYTES_THRESHOLD, DEFAULT_BULK_ROW_THRESHOLD};
    use crate::storage::tenant::context::TenantContext;
    use proximadb_catalog::{CatalogStorageSpecialization, CatalogTableSchema};
    use std::sync::Mutex;

    struct VecSourceReader {
        batches: Mutex<Vec<Vec<ProximaRecord>>>,
    }

    impl VecSourceReader {
        fn new(mut batches: Vec<Vec<ProximaRecord>>) -> Self {
            batches.reverse();
            Self {
                batches: Mutex::new(batches),
            }
        }
    }

    #[async_trait]
    impl TableRecordSourceReader for VecSourceReader {
        async fn next_batch(
            &self,
            _source: &ReadSource,
            _source_schema: Option<&CatalogTableSchema>,
            _target_schema: &CatalogTableSchema,
            _cursor: &mut TableRecordSourceCursor,
        ) -> Result<Option<Vec<ProximaRecord>>> {
            Ok(self.batches.lock().unwrap().pop())
        }
    }

    struct CapturingRecordStore {
        writes: Mutex<Vec<TableRecordMutation>>,
    }

    #[async_trait]
    impl TableRecordStore for CapturingRecordStore {
        async fn write_mutations(
            &self,
            _table_schema: &CatalogTableSchema,
            mutations: Vec<TableRecordMutation>,
            _tenant_context: Option<&TenantContext>,
        ) -> Result<TableRecordWriteResult> {
            let ids = mutations
                .iter()
                .map(|mutation| mutation.record.oid.clone())
                .collect::<Vec<_>>();
            self.writes.lock().unwrap().extend(mutations);
            Ok(TableRecordWriteResult {
                success: true,
                record_ids: ids,
                metrics: OperationMetrics::default(),
                errors: Vec::new(),
                error_code: None,
            })
        }

        async fn get_by_key(
            &self,
            _table_schema: &CatalogTableSchema,
            _request: TableRecordGetRequest,
            _tenant_context: Option<&TenantContext>,
        ) -> Result<TableRecordGetResponse> {
            Ok(None)
        }
    }

    struct ScanRecordStore {
        records: Vec<ProximaRecord>,
    }

    #[async_trait]
    impl TableRecordStore for ScanRecordStore {
        async fn write_mutations(
            &self,
            _table_schema: &CatalogTableSchema,
            _mutations: Vec<TableRecordMutation>,
            _tenant_context: Option<&TenantContext>,
        ) -> Result<TableRecordWriteResult> {
            Ok(TableRecordWriteResult {
                success: true,
                record_ids: Vec::new(),
                metrics: OperationMetrics::default(),
                errors: Vec::new(),
                error_code: None,
            })
        }

        async fn get_by_key(
            &self,
            _table_schema: &CatalogTableSchema,
            _request: TableRecordGetRequest,
            _tenant_context: Option<&TenantContext>,
        ) -> Result<TableRecordGetResponse> {
            Ok(None)
        }

        async fn scan_records(
            &self,
            _table_schema: &CatalogTableSchema,
            _request: TableRecordScanRequest,
            _tenant_context: Option<&TenantContext>,
        ) -> Result<Vec<ProximaRecord>> {
            Ok(self.records.clone())
        }
    }

    fn test_record(id: &str) -> ProximaRecord {
        ProximaRecord {
            oid: id.to_string(),
            local_id: Some(id.to_string()),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn planned_executor_reports_selected_route_without_committing() {
        let schema = CatalogTableSchema::new("orders")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);
        let plan =
            CopyIntoPlan::insert_select(LogicalTableRef::new("orders"), "SELECT * FROM staging");
        let routed =
            TableWriteRouter::default().route(crate::query::table_write_plan::RoutingContext {
                target_schema: &schema,
                target_stats: None,
                source_schema: None,
                source_stats: None,
                write_intent_overrides: None,
                plan: &plan,
            });

        let result = PlannedOnlyTableWriteExecutor::new()
            .execute(TableWriteExecutionRequest {
                target_schema: &schema,
                source_schema: None,
                routed_plan: routed,
            })
            .await
            .unwrap();

        assert_eq!(result.status, TableWriteExecutionStatus::PlannedOnly);
        assert_eq!(result.rows_written, 0);
        assert!(result.route_summary.contains("backend=Native"));
    }

    #[tokio::test]
    async fn planned_executor_rejects_routes_missing_core_guards() {
        let schema = CatalogTableSchema::new("orders");
        let mut routed =
            TableWriteRouter::default().route(crate::query::table_write_plan::RoutingContext {
                target_schema: &schema,
                target_stats: None,
                source_schema: None,
                source_stats: None,
                write_intent_overrides: None,
                plan: &CopyIntoPlan {
                    source: crate::query::table_write_plan::ReadSource::QuerySql(
                        "SELECT * FROM staging".to_string(),
                    ),
                    target: LogicalTableRef::new("orders"),
                    write_mode: WriteMode::Append,
                    conflict_policy: Default::default(),
                    distribution: Default::default(),
                },
            });
        routed.required_guards.clear();
        routed.selected_path.guards.clear();

        let err = PlannedOnlyTableWriteExecutor::new()
            .execute(TableWriteExecutionRequest {
                target_schema: &schema,
                source_schema: None,
                routed_plan: RoutedExecutionPlan {
                    backend: ComputeBackend::Native,
                    estimated_cost: CostEstimate {
                        rows: None,
                        bytes: None,
                        relative_cost: 1.0,
                        reason: "test".to_string(),
                    },
                    ..routed
                },
            })
            .await
            .unwrap_err();

        assert!(err.to_string().contains("missing required guard"));
    }

    #[tokio::test]
    async fn native_source_reader_classifies_unsupported_sources() {
        let reader = TableRecordStoreSourceReader::new(Arc::new(ScanRecordStore {
            records: Vec::new(),
        }));
        let schema = CatalogTableSchema::new("orders");
        let mut cursor = TableRecordSourceCursor::default();

        let err = reader
            .next_batch(
                &ReadSource::QuerySql("SELECT * FROM staging".to_string()),
                None,
                &schema,
                &mut cursor,
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("PermanentReadFailure"));
        assert!(err.to_string().contains("requires a compute adapter"));
    }

    #[tokio::test]
    async fn native_source_reader_classifies_schema_mismatch() {
        let reader = TableRecordStoreSourceReader::new(Arc::new(ScanRecordStore {
            records: Vec::new(),
        }));
        let target_schema = CatalogTableSchema::new("orders");
        let source_schema = CatalogTableSchema::new("staging");
        let mut cursor = TableRecordSourceCursor::default();

        let err = reader
            .next_batch(
                &ReadSource::CatalogTable {
                    table: LogicalTableRef::new("orders"),
                    snapshot: Default::default(),
                },
                Some(&source_schema),
                &target_schema,
                &mut cursor,
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("TypeViolation"));
        assert!(err.to_string().contains("does not match resolved schema"));
    }

    #[tokio::test]
    async fn native_executor_writes_source_batches_through_record_store() {
        let schema = CatalogTableSchema::new("orders")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);
        let plan =
            CopyIntoPlan::insert_select(LogicalTableRef::new("orders"), "SELECT * FROM staging");
        let routed =
            TableWriteRouter::default().route(crate::query::table_write_plan::RoutingContext {
                target_schema: &schema,
                target_stats: None,
                source_schema: None,
                source_stats: None,
                write_intent_overrides: None,
                plan: &plan,
            });
        assert_eq!(routed.backend, ComputeBackend::Native);

        let source = Arc::new(VecSourceReader::new(vec![
            vec![test_record("r1"), test_record("r2")],
            vec![test_record("r3")],
        ]));
        let store = Arc::new(CapturingRecordStore {
            writes: Mutex::new(Vec::new()),
        });

        let result = NativeTableWriteExecutor::new(source, store.clone())
            .execute(TableWriteExecutionRequest {
                target_schema: &schema,
                source_schema: None,
                routed_plan: routed,
            })
            .await
            .unwrap();

        assert_eq!(result.status, TableWriteExecutionStatus::Completed);
        assert_eq!(result.rows_written, 3);
        let writes = store.writes.lock().unwrap();
        assert_eq!(writes.len(), 3);
        assert!(
            writes
                .iter()
                .all(|mutation| mutation.kind == TableRecordMutationKind::Insert)
        );
    }

    #[tokio::test]
    async fn table_record_store_source_reader_batches_catalog_table_scans() {
        let source_schema = CatalogTableSchema::new("staging")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);
        let target_schema = CatalogTableSchema::new("orders")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);
        let source = Arc::new(ScanRecordStore {
            records: vec![test_record("r1"), test_record("r2"), test_record("r3")],
        });
        let reader = TableRecordStoreSourceReader::with_batch_size(source, 2);
        let read_source = ReadSource::CatalogTable {
            table: crate::query::table_write_plan::LogicalTableRef::new("staging"),
            snapshot: Default::default(),
        };
        let mut cursor = TableRecordSourceCursor::default();

        let first = reader
            .next_batch(
                &read_source,
                Some(&source_schema),
                &target_schema,
                &mut cursor,
            )
            .await
            .unwrap()
            .expect("first batch");
        let second = reader
            .next_batch(
                &read_source,
                Some(&source_schema),
                &target_schema,
                &mut cursor,
            )
            .await
            .unwrap()
            .expect("second batch");
        let done = reader
            .next_batch(
                &read_source,
                Some(&source_schema),
                &target_schema,
                &mut cursor,
            )
            .await
            .unwrap();

        assert_eq!(first.len(), 2);
        assert_eq!(second.len(), 1);
        assert!(done.is_none());
    }

    #[tokio::test]
    async fn native_executor_rejects_merge_until_predicates_exist() {
        let schema = CatalogTableSchema::new("orders")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);
        let mut plan =
            CopyIntoPlan::insert_select(LogicalTableRef::new("orders"), "SELECT * FROM staging");
        plan.write_mode = WriteMode::Merge;
        let routed =
            TableWriteRouter::default().route(crate::query::table_write_plan::RoutingContext {
                target_schema: &schema,
                target_stats: None,
                source_schema: None,
                source_stats: None,
                write_intent_overrides: None,
                plan: &plan,
            });

        let err = NativeTableWriteExecutor::new(
            Arc::new(VecSourceReader::new(vec![vec![test_record("r1")]])),
            Arc::new(CapturingRecordStore {
                writes: Mutex::new(Vec::new()),
            }),
        )
        .execute(TableWriteExecutionRequest {
            target_schema: &schema,
            source_schema: None,
            routed_plan: routed,
        })
        .await
        .unwrap_err();

        assert!(err.to_string().contains("MERGE table-write execution"));
    }

    struct FailingRecordStore;

    #[async_trait]
    impl TableRecordStore for FailingRecordStore {
        async fn write_mutations(
            &self,
            _table_schema: &CatalogTableSchema,
            _mutations: Vec<TableRecordMutation>,
            _tenant_context: Option<&TenantContext>,
        ) -> Result<TableRecordWriteResult> {
            Ok(TableRecordWriteResult {
                success: false,
                record_ids: Vec::new(),
                metrics: OperationMetrics::default(),
                errors: vec!["simulated write failure".to_string()],
                error_code: None,
            })
        }

        async fn get_by_key(
            &self,
            _table_schema: &CatalogTableSchema,
            _request: TableRecordGetRequest,
            _tenant_context: Option<&TenantContext>,
        ) -> Result<TableRecordGetResponse> {
            Ok(None)
        }

        async fn scan_records(
            &self,
            _table_schema: &CatalogTableSchema,
            _request: TableRecordScanRequest,
            _tenant_context: Option<&TenantContext>,
        ) -> Result<Vec<ProximaRecord>> {
            Ok(Vec::new())
        }
    }

    #[tokio::test]
    async fn native_executor_empty_source_completes_with_zero_rows() {
        let schema = CatalogTableSchema::new("orders")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);
        let plan =
            CopyIntoPlan::insert_select(LogicalTableRef::new("orders"), "SELECT * FROM staging");
        let routed =
            TableWriteRouter::default().route(crate::query::table_write_plan::RoutingContext {
                target_schema: &schema,
                target_stats: None,
                source_schema: None,
                source_stats: None,
                write_intent_overrides: None,
                plan: &plan,
            });

        let result = NativeTableWriteExecutor::new(
            Arc::new(VecSourceReader::new(vec![])),
            Arc::new(CapturingRecordStore {
                writes: Mutex::new(Vec::new()),
            }),
        )
        .execute(TableWriteExecutionRequest {
            target_schema: &schema,
            source_schema: None,
            routed_plan: routed,
        })
        .await
        .unwrap();

        assert_eq!(result.status, TableWriteExecutionStatus::Completed);
        assert_eq!(result.rows_written, 0);
    }

    #[tokio::test]
    async fn native_executor_propagates_store_write_failure() {
        let schema = CatalogTableSchema::new("orders")
            .with_storage_specialization(CatalogStorageSpecialization::PaxOltp);
        let plan =
            CopyIntoPlan::insert_select(LogicalTableRef::new("orders"), "SELECT * FROM staging");
        let routed =
            TableWriteRouter::default().route(crate::query::table_write_plan::RoutingContext {
                target_schema: &schema,
                target_stats: None,
                source_schema: None,
                source_stats: None,
                write_intent_overrides: None,
                plan: &plan,
            });

        let err = NativeTableWriteExecutor::new(
            Arc::new(VecSourceReader::new(vec![vec![test_record("r1")]])),
            Arc::new(FailingRecordStore),
        )
        .execute(TableWriteExecutionRequest {
            target_schema: &schema,
            source_schema: None,
            routed_plan: routed,
        })
        .await
        .unwrap_err();

        assert!(err.to_string().contains("Native table write failed"));
        assert!(err.to_string().contains("simulated write failure"));
    }

    #[tokio::test]
    async fn native_executor_rejects_direct_commit_lanes_until_implemented() {
        let schema = CatalogTableSchema::new("facts")
            .with_storage_specialization(CatalogStorageSpecialization::ColumnarAnalytics);
        let plan =
            CopyIntoPlan::insert_select(LogicalTableRef::new("facts"), "SELECT * FROM staging");
        let overrides = WriteIntentOverrides {
            row_count_hint: Some(DEFAULT_BULK_ROW_THRESHOLD),
            estimated_bytes: Some(DEFAULT_BULK_BYTES_THRESHOLD),
            batch_local_constraints_sufficient: Some(true),
            ..Default::default()
        };
        let mut routed =
            TableWriteRouter::default().route(crate::query::table_write_plan::RoutingContext {
                target_schema: &schema,
                target_stats: None,
                source_schema: None,
                source_stats: None,
                write_intent_overrides: Some(&overrides),
                plan: &plan,
            });
        assert_eq!(routed.write_lane_decision.lane, WriteLane::BulkAppendCommit);
        routed.backend = ComputeBackend::Native;

        let err = NativeTableWriteExecutor::new(
            Arc::new(VecSourceReader::new(vec![vec![test_record("r1")]])),
            Arc::new(CapturingRecordStore {
                writes: Mutex::new(Vec::new()),
            }),
        )
        .execute(TableWriteExecutionRequest {
            target_schema: &schema,
            source_schema: None,
            routed_plan: routed,
        })
        .await
        .unwrap_err();

        assert!(err.to_string().contains("BulkAppendCommit"));
    }
}
