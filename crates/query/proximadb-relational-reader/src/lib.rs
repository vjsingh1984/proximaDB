//! Storage adapter trait for ProximaDB's relational query path
//! (ADR-019 L5 / S4). Each storage engine (SST, VIPER, NOVA, future
//! external readers) implements [`RelationalReader`] and declares
//! its [`ReaderCapabilities`]. The planner consults capabilities
//! to decide what stays in the executor vs what pushes down.
//!
//! Contract overview:
//!
//! - [`RelationalReader`] is async because real storage engines
//!   need to fetch blocks off disk or over the network.
//! - Each reader yields rows one at a time via `next_row`. Batch
//!   APIs are a Phase 3 optimisation; the trait stays stable.
//! - [`ScanContext`] carries the planner's hints (projection,
//!   predicate, snapshot). The adapter MAY ignore predicate
//!   pushdown — the executor always re-applies the predicate per
//!   row for correctness.
//! - [`ReaderCapabilities`] is the adapter's declaration of what
//!   it can do efficiently. The planner uses this to decide
//!   whether to bother passing a predicate (cheap if used for
//!   block skipping; wasteful otherwise).
//! - Primary-key lookup is a separate API path
//!   ([`RelationalReader::lookup_pk`]) because it has a different
//!   cost shape than a scan + filter.
//!
//! See [`VecReader`] for a reference in-memory implementation that
//! satisfies every capability flag. Real engine adapters (SST,
//! VIPER, NOVA) live in the root crate and wrap their engine's
//! reader API.

use async_trait::async_trait;
use proximadb_data_model::ProximaValue;
use proximadb_relational_types::{Expr, ExprError, NoFunctions, RelationalRow, RelationalSchema};
use serde::{Deserialize, Serialize};
use thiserror::Error;

// =========================================================================
// Errors
// =========================================================================

#[derive(Debug, Error, Clone, PartialEq)]
pub enum ReaderError {
    #[error("reader not opened — call open(ctx) before next_row()")]
    NotOpen,

    #[error("invalid projection: {0}")]
    InvalidProjection(String),

    #[error("predicate evaluation failed: {0}")]
    PredicateEval(#[from] ExprError),

    #[error("primary-key lookup is not supported by this reader")]
    PkLookupUnsupported,

    #[error("primary-key arity mismatch: expected {expected}, got {actual}")]
    PkArityMismatch { expected: usize, actual: usize },

    #[error("storage error: {0}")]
    Storage(String),

    #[error("scan cancelled")]
    Cancelled,

    #[error("{0}")]
    Other(String),
}

// =========================================================================
// Snapshot + scan context
// =========================================================================

/// Point-in-time snapshot the reader should respect. Set by the
/// transaction layer (S6) at session BEGIN; passed unchanged
/// through the planner to every scan in the plan. Adapters that
/// don't yet support snapshot reads ignore this and read latest.
///
/// `lsn = u64::MAX` means "read latest" (autocommit / outside
/// transaction).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReadSnapshot {
    pub lsn: u64,
    /// Wall-clock at which the snapshot was taken, in millis since
    /// epoch. Carried for diagnostics; not used for correctness.
    pub taken_at_ms: i64,
}

impl ReadSnapshot {
    pub const fn latest() -> Self {
        Self {
            lsn: u64::MAX,
            taken_at_ms: 0,
        }
    }

    pub fn at_lsn(lsn: u64, taken_at_ms: i64) -> Self {
        Self { lsn, taken_at_ms }
    }
}

impl Default for ReadSnapshot {
    fn default() -> Self {
        Self::latest()
    }
}

/// Planner-supplied scan inputs. Adapters use this to decide what
/// to fetch and decode. Every field is optional except the
/// snapshot — the planner's defaults degrade to "scan everything".
#[derive(Debug, Clone, Default)]
pub struct ScanContext {
    /// Ordered list of columns to emit. `None` = emit all columns
    /// in the reader's declared schema order. Names are matched
    /// against the reader's schema; unknown names cause
    /// `InvalidProjection`.
    pub projection: Option<Vec<String>>,

    /// Filter predicate. The adapter MAY use this for
    /// block/segment skipping based on its
    /// [`ReaderCapabilities::push_predicate`] flag. The executor
    /// always re-applies the predicate per row for correctness.
    pub predicate: Option<Expr>,

    /// Optional row limit. The adapter MAY stop early; the
    /// executor enforces the final limit defensively.
    pub limit: Option<u64>,

    /// Visibility snapshot.
    pub snapshot: ReadSnapshot,
}

// =========================================================================
// Capabilities
// =========================================================================

/// What this reader can do. The planner reads these flags to decide
/// what to push down. Falsy flags do NOT mean the operation is
/// unsupported — they mean the adapter will treat it as a no-op
/// (the executor still handles the operation in Rust). The flags
/// are strictly an efficiency signal.
///
/// Exception: `pk_lookup`. If false, calls to
/// [`RelationalReader::lookup_pk`] return
/// [`ReaderError::PkLookupUnsupported`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct ReaderCapabilities {
    /// Reader will only decode the columns named in
    /// `ScanContext::projection`. Saves CPU + memory on wide
    /// tables.
    pub project_columns: bool,

    /// Reader will use `ScanContext::predicate` for block-level
    /// skipping (zone-map / bloom). Reader still emits per-block
    /// rows; the executor re-filters per row.
    pub push_predicate: bool,

    /// Reader supports primary-key lookup via
    /// [`RelationalReader::lookup_pk`] (vs returning the
    /// `PkLookupUnsupported` error).
    pub pk_lookup: bool,

    /// Reader has bloom filters on the columns the planner is
    /// filtering against. Used for equality probes.
    pub bloom_probe: bool,

    /// Reader has zone maps (min/max per block) for range
    /// predicates.
    pub zone_map_skip: bool,
}

impl ReaderCapabilities {
    /// No capabilities — every operation runs in the executor.
    /// The planner still passes capabilities through; this is the
    /// safe default for a brand-new adapter.
    pub const fn none() -> Self {
        Self {
            project_columns: false,
            push_predicate: false,
            pk_lookup: false,
            bloom_probe: false,
            zone_map_skip: false,
        }
    }

    /// All capabilities. The reference [`VecReader`] satisfies
    /// these for testability; real engines pick à la carte.
    pub const fn full() -> Self {
        Self {
            project_columns: true,
            push_predicate: true,
            pk_lookup: true,
            bloom_probe: true,
            zone_map_skip: true,
        }
    }

    pub const fn with_project_columns(mut self, b: bool) -> Self {
        self.project_columns = b;
        self
    }
    pub const fn with_push_predicate(mut self, b: bool) -> Self {
        self.push_predicate = b;
        self
    }
    pub const fn with_pk_lookup(mut self, b: bool) -> Self {
        self.pk_lookup = b;
        self
    }
    pub const fn with_bloom_probe(mut self, b: bool) -> Self {
        self.bloom_probe = b;
        self
    }
    pub const fn with_zone_map_skip(mut self, b: bool) -> Self {
        self.zone_map_skip = b;
        self
    }
}

// =========================================================================
// The trait
// =========================================================================

/// Engine-agnostic relational reader. Built once per
/// `Scan` plan node; the executor drives it through the open →
/// next_row → … → close lifecycle.
///
/// Implementations are responsible for:
///
/// - Persisting the open context internally until close.
/// - Returning rows in the schema declared by [`Self::schema`],
///   trimmed to the projection if the adapter declares
///   `project_columns = true`.
/// - Honouring the snapshot LSN if supported; reading latest if
///   not.
///
/// Implementations are NOT responsible for:
///
/// - Re-evaluating the predicate per row (the executor does that).
/// - Enforcing the limit (the executor does that).
/// - Handling NULL semantics (the executor does that).
#[async_trait]
pub trait RelationalReader: Send {
    /// Stable identifier for logs and metrics, e.g. `"sst"`,
    /// `"viper"`, `"nova"`, `"vec"` (test).
    fn name(&self) -> &'static str;

    /// What this reader can do efficiently.
    fn capabilities(&self) -> ReaderCapabilities;

    /// The reader's declared output schema (after applying any
    /// `project_columns = true` projection from the most recent
    /// `open` call). The executor uses this to lay out output
    /// rows.
    fn schema(&self) -> &RelationalSchema;

    /// Open the reader for a fresh scan. Idempotent if called
    /// twice with the same context. Returns `InvalidProjection`
    /// if a projection column name doesn't exist in the
    /// underlying schema.
    async fn open(&mut self, ctx: &ScanContext) -> Result<(), ReaderError>;

    /// Pull the next row. `Ok(None)` is EOF — no more rows in
    /// this scan. The reader stays open for any post-EOF
    /// telemetry queries (`schema`, `capabilities`); call
    /// `close` to release resources.
    async fn next_row(&mut self) -> Result<Option<RelationalRow>, ReaderError>;

    /// Fast-path primary-key lookup. Returns
    /// [`ReaderError::PkLookupUnsupported`] when
    /// `capabilities().pk_lookup == false`.
    ///
    /// The key arity must match the schema's primary-key column
    /// count; the adapter validates and returns
    /// [`ReaderError::PkArityMismatch`] otherwise.
    async fn lookup_pk(
        &self,
        key: &[ProximaValue],
    ) -> Result<Option<RelationalRow>, ReaderError>;

    /// Release any resources. Idempotent; safe to call even if
    /// `open` was never called.
    async fn close(&mut self) -> Result<(), ReaderError>;
}

// =========================================================================
// Reference impl: in-memory VecReader
// =========================================================================

/// In-memory reference reader backed by a static `Vec<RelationalRow>`.
/// Supports every capability flag for testability:
///
/// - `project_columns`: yes — emits only requested columns.
/// - `push_predicate`: yes — evaluates the predicate row-by-row
///   internally (no block-skip optimisation; the adapter has one
///   block: the full vec).
/// - `pk_lookup`: yes — linear scan over `pk_columns` for the
///   matching row.
/// - `bloom_probe` / `zone_map_skip`: declared but no-op (the
///   single in-memory block has no skip metadata).
///
/// Real engine adapters (SST, VIPER, NOVA) follow the same shape
/// but plug into their engine's storage primitives.
pub struct VecReader {
    full_schema: RelationalSchema,
    rows: Vec<RelationalRow>,
    pk_columns: Vec<usize>,
    open_ctx: Option<OpenState>,
}

struct OpenState {
    /// Schema as seen by the caller (after projection).
    output_schema: RelationalSchema,
    /// Indices in `full_schema` to emit for each output column.
    projection_indices: Vec<usize>,
    predicate: Option<Expr>,
    limit: Option<u64>,
    cursor: usize,
    rows_emitted: u64,
}

impl VecReader {
    /// Build a reader from rows + schema. `pk_columns` lists the
    /// ordinals (within `schema`) that form the primary key.
    /// Pass an empty Vec if the table has no PK (PK lookups will
    /// then return `PkLookupUnsupported`).
    pub fn new(
        schema: RelationalSchema,
        rows: Vec<RelationalRow>,
        pk_columns: Vec<usize>,
    ) -> Self {
        Self {
            full_schema: schema,
            rows,
            pk_columns,
            open_ctx: None,
        }
    }

    fn resolve_projection(
        &self,
        projection: &Option<Vec<String>>,
    ) -> Result<(RelationalSchema, Vec<usize>), ReaderError> {
        let Some(names) = projection else {
            // No projection: emit full schema.
            let indices = (0..self.full_schema.len()).collect();
            return Ok((self.full_schema.clone(), indices));
        };
        let mut cols = Vec::with_capacity(names.len());
        let mut indices = Vec::with_capacity(names.len());
        for n in names {
            let (idx, info) = self
                .full_schema
                .column_by_name(n)
                .ok_or_else(|| ReaderError::InvalidProjection(n.clone()))?;
            cols.push(info.clone());
            indices.push(idx);
        }
        Ok((RelationalSchema::new(cols), indices))
    }

    fn project_row(&self, row: &RelationalRow, indices: &[usize]) -> RelationalRow {
        indices.iter().map(|&i| row[i].clone()).collect()
    }
}

#[async_trait]
impl RelationalReader for VecReader {
    fn name(&self) -> &'static str {
        "vec"
    }

    fn capabilities(&self) -> ReaderCapabilities {
        ReaderCapabilities::full().with_pk_lookup(!self.pk_columns.is_empty())
    }

    fn schema(&self) -> &RelationalSchema {
        match &self.open_ctx {
            Some(state) => &state.output_schema,
            None => &self.full_schema,
        }
    }

    async fn open(&mut self, ctx: &ScanContext) -> Result<(), ReaderError> {
        let (output_schema, projection_indices) =
            self.resolve_projection(&ctx.projection)?;
        // Type-check the predicate against the FULL schema (the
        // predicate may reference unprojected columns even when
        // projection is narrower).
        if let Some(pred) = &ctx.predicate {
            pred.type_check(&self.full_schema)
                .map_err(ReaderError::PredicateEval)?;
        }
        self.open_ctx = Some(OpenState {
            output_schema,
            projection_indices,
            predicate: ctx.predicate.clone(),
            limit: ctx.limit,
            cursor: 0,
            rows_emitted: 0,
        });
        Ok(())
    }

    async fn next_row(&mut self) -> Result<Option<RelationalRow>, ReaderError> {
        // Pull the state out first so we don't hold &mut self.open_ctx
        // across &self.rows accesses (Rust's borrow checker disallows
        // simultaneous mutable + immutable borrows of `self`).
        let state = self.open_ctx.as_mut().ok_or(ReaderError::NotOpen)?;
        loop {
            if state.cursor >= self.rows.len() {
                return Ok(None);
            }
            if let Some(limit) = state.limit {
                if state.rows_emitted >= limit {
                    return Ok(None);
                }
            }
            let row = &self.rows[state.cursor];
            state.cursor += 1;
            // Evaluate the pushed-down predicate against the FULL
            // row (not projected) so it can reference columns that
            // aren't in the projection.
            if let Some(pred) = &state.predicate {
                let v = pred
                    .eval(row, &NoFunctions)
                    .map_err(ReaderError::PredicateEval)?;
                if !matches!(v, ProximaValue::Boolean(true)) {
                    continue;
                }
            }
            let projected: RelationalRow = state
                .projection_indices
                .iter()
                .map(|&i| row[i].clone())
                .collect();
            state.rows_emitted += 1;
            return Ok(Some(projected));
        }
    }

    async fn lookup_pk(
        &self,
        key: &[ProximaValue],
    ) -> Result<Option<RelationalRow>, ReaderError> {
        if self.pk_columns.is_empty() {
            return Err(ReaderError::PkLookupUnsupported);
        }
        if key.len() != self.pk_columns.len() {
            return Err(ReaderError::PkArityMismatch {
                expected: self.pk_columns.len(),
                actual: key.len(),
            });
        }
        for row in &self.rows {
            let matches = self
                .pk_columns
                .iter()
                .zip(key.iter())
                .all(|(&idx, expected)| &row[idx] == expected);
            if matches {
                return Ok(Some(row.clone()));
            }
        }
        Ok(None)
    }

    async fn close(&mut self) -> Result<(), ReaderError> {
        self.open_ctx = None;
        Ok(())
    }
}

// =========================================================================
// Helpers for test reader construction
// =========================================================================

/// Collect every row from a freshly-opened reader. Useful in the
/// executor and in tests. Stops when `next_row` returns `Ok(None)`.
pub async fn drain_all<R: RelationalReader + ?Sized>(
    reader: &mut R,
) -> Result<Vec<RelationalRow>, ReaderError> {
    let mut out = Vec::new();
    while let Some(row) = reader.next_row().await? {
        out.push(row);
    }
    Ok(out)
}

// =========================================================================
// Tests
// =========================================================================

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_data_model::ProximaType;
    use proximadb_relational_types::{BinaryOp, ColumnInfo};

    fn users_schema() -> RelationalSchema {
        RelationalSchema::new(vec![
            ColumnInfo::new("id", ProximaType::Int64, false),
            ColumnInfo::new("name", ProximaType::String, true),
            ColumnInfo::new("age", ProximaType::Int32, true),
        ])
    }

    fn users_rows() -> Vec<RelationalRow> {
        vec![
            vec![
                ProximaValue::Int64(1),
                ProximaValue::String("alice".into()),
                ProximaValue::Int32(30),
            ],
            vec![
                ProximaValue::Int64(2),
                ProximaValue::String("bob".into()),
                ProximaValue::Int32(25),
            ],
            vec![
                ProximaValue::Int64(3),
                ProximaValue::String("carol".into()),
                ProximaValue::Int32(40),
            ],
        ]
    }

    fn users_reader() -> VecReader {
        VecReader::new(users_schema(), users_rows(), vec![0]) // PK on id
    }

    // --- Capabilities ---------------------------------------------

    #[test]
    fn capabilities_default_is_none() {
        let c = ReaderCapabilities::default();
        assert!(!c.project_columns);
        assert!(!c.push_predicate);
        assert!(!c.pk_lookup);
        assert!(!c.bloom_probe);
        assert!(!c.zone_map_skip);
    }

    #[test]
    fn capabilities_full_sets_everything() {
        let c = ReaderCapabilities::full();
        assert!(c.project_columns);
        assert!(c.push_predicate);
        assert!(c.pk_lookup);
        assert!(c.bloom_probe);
        assert!(c.zone_map_skip);
    }

    #[test]
    fn capabilities_builder_is_chainable() {
        let c = ReaderCapabilities::none()
            .with_project_columns(true)
            .with_push_predicate(true);
        assert!(c.project_columns);
        assert!(c.push_predicate);
        assert!(!c.pk_lookup);
    }

    // --- Snapshot --------------------------------------------------

    #[test]
    fn snapshot_latest_means_max_lsn() {
        let s = ReadSnapshot::latest();
        assert_eq!(s.lsn, u64::MAX);
    }

    // --- VecReader: name + capabilities ---------------------------

    #[test]
    fn vec_reader_reports_name_and_capabilities() {
        let r = users_reader();
        assert_eq!(r.name(), "vec");
        let c = r.capabilities();
        assert!(c.project_columns);
        assert!(c.push_predicate);
        assert!(c.pk_lookup);
    }

    #[test]
    fn vec_reader_pk_lookup_off_when_no_pk() {
        let r = VecReader::new(users_schema(), users_rows(), vec![]);
        let c = r.capabilities();
        assert!(!c.pk_lookup);
    }

    // --- VecReader: lifecycle -------------------------------------

    #[tokio::test]
    async fn vec_reader_next_row_before_open_errors() {
        let mut r = users_reader();
        let err = r.next_row().await.unwrap_err();
        assert!(matches!(err, ReaderError::NotOpen));
    }

    #[tokio::test]
    async fn vec_reader_open_scan_close_emits_all_rows() {
        let mut r = users_reader();
        r.open(&ScanContext::default()).await.unwrap();
        let rows = drain_all(&mut r).await.unwrap();
        assert_eq!(rows.len(), 3);
        r.close().await.unwrap();
    }

    #[tokio::test]
    async fn vec_reader_close_is_idempotent() {
        let mut r = users_reader();
        r.close().await.unwrap();
        r.close().await.unwrap();
    }

    // --- Projection ----------------------------------------------

    #[tokio::test]
    async fn vec_reader_projection_emits_only_requested_columns() {
        let mut r = users_reader();
        let ctx = ScanContext {
            projection: Some(vec!["name".into(), "id".into()]),
            ..ScanContext::default()
        };
        r.open(&ctx).await.unwrap();
        let rows = drain_all(&mut r).await.unwrap();
        assert_eq!(rows.len(), 3);
        // 2 columns, reordered: name first, id second.
        assert_eq!(rows[0].len(), 2);
        assert_eq!(rows[0][0], ProximaValue::String("alice".into()));
        assert_eq!(rows[0][1], ProximaValue::Int64(1));
        // Schema reflects projection.
        assert_eq!(r.schema().columns[0].name, "name");
        assert_eq!(r.schema().columns[1].name, "id");
    }

    #[tokio::test]
    async fn vec_reader_unknown_projection_column_errors() {
        let mut r = users_reader();
        let ctx = ScanContext {
            projection: Some(vec!["nope".into()]),
            ..ScanContext::default()
        };
        let err = r.open(&ctx).await.unwrap_err();
        assert!(matches!(err, ReaderError::InvalidProjection(_)));
    }

    // --- Predicate pushdown --------------------------------------

    #[tokio::test]
    async fn vec_reader_predicate_filters_rows() {
        let mut r = users_reader();
        let schema = users_schema();
        let age = schema.resolve_column("age").unwrap();
        let predicate = Expr::bin(
            BinaryOp::Gt,
            Expr::column(age),
            Expr::literal(ProximaValue::Int32(29)),
        );
        let ctx = ScanContext {
            predicate: Some(predicate),
            ..ScanContext::default()
        };
        r.open(&ctx).await.unwrap();
        let rows = drain_all(&mut r).await.unwrap();
        // alice(30) and carol(40); bob(25) excluded.
        assert_eq!(rows.len(), 2);
        let names: Vec<String> = rows
            .iter()
            .map(|r| match &r[1] {
                ProximaValue::String(s) => s.clone(),
                _ => unreachable!(),
            })
            .collect();
        assert!(names.contains(&"alice".to_string()));
        assert!(names.contains(&"carol".to_string()));
    }

    #[tokio::test]
    async fn vec_reader_predicate_can_reference_unprojected_column() {
        // SELECT name FROM users WHERE age > 29
        let mut r = users_reader();
        let schema = users_schema();
        let age = schema.resolve_column("age").unwrap();
        let predicate = Expr::bin(
            BinaryOp::Gt,
            Expr::column(age),
            Expr::literal(ProximaValue::Int32(29)),
        );
        let ctx = ScanContext {
            projection: Some(vec!["name".into()]),
            predicate: Some(predicate),
            ..ScanContext::default()
        };
        r.open(&ctx).await.unwrap();
        let rows = drain_all(&mut r).await.unwrap();
        // 2 rows, projected to just `name`.
        assert_eq!(rows.len(), 2);
        for row in &rows {
            assert_eq!(row.len(), 1);
            assert!(matches!(row[0], ProximaValue::String(_)));
        }
    }

    #[tokio::test]
    async fn vec_reader_predicate_type_check_failure_errors_at_open() {
        let mut r = users_reader();
        let predicate = Expr::Column(proximadb_relational_types::ColumnRef {
            name: "id".into(),
            ordinal: 0,
            ty: ProximaType::String, // wrong — actual is Int64
            nullable: false,
        });
        let ctx = ScanContext {
            predicate: Some(predicate),
            ..ScanContext::default()
        };
        let err = r.open(&ctx).await.unwrap_err();
        assert!(matches!(err, ReaderError::PredicateEval(_)));
    }

    // --- Limit ----------------------------------------------------

    #[tokio::test]
    async fn vec_reader_respects_limit() {
        let mut r = users_reader();
        let ctx = ScanContext {
            limit: Some(2),
            ..ScanContext::default()
        };
        r.open(&ctx).await.unwrap();
        let rows = drain_all(&mut r).await.unwrap();
        assert_eq!(rows.len(), 2);
    }

    #[tokio::test]
    async fn vec_reader_limit_zero_emits_nothing() {
        let mut r = users_reader();
        let ctx = ScanContext {
            limit: Some(0),
            ..ScanContext::default()
        };
        r.open(&ctx).await.unwrap();
        let rows = drain_all(&mut r).await.unwrap();
        assert_eq!(rows.len(), 0);
    }

    // --- PK lookup ------------------------------------------------

    #[tokio::test]
    async fn vec_reader_pk_lookup_finds_row() {
        let r = users_reader();
        let row = r.lookup_pk(&[ProximaValue::Int64(2)]).await.unwrap().unwrap();
        assert_eq!(row[1], ProximaValue::String("bob".into()));
    }

    #[tokio::test]
    async fn vec_reader_pk_lookup_returns_none_for_missing() {
        let r = users_reader();
        let result = r.lookup_pk(&[ProximaValue::Int64(999)]).await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn vec_reader_pk_lookup_unsupported_when_no_pk() {
        let r = VecReader::new(users_schema(), users_rows(), vec![]);
        let err = r.lookup_pk(&[ProximaValue::Int64(1)]).await.unwrap_err();
        assert!(matches!(err, ReaderError::PkLookupUnsupported));
    }

    #[tokio::test]
    async fn vec_reader_pk_lookup_arity_mismatch_errors() {
        let r = users_reader();
        let err = r
            .lookup_pk(&[ProximaValue::Int64(1), ProximaValue::Int64(2)])
            .await
            .unwrap_err();
        assert!(matches!(err, ReaderError::PkArityMismatch { .. }));
    }

    // --- Trait object dispatch -----------------------------------

    #[tokio::test]
    async fn reader_can_be_used_as_trait_object() {
        // The executor will hold a Box<dyn RelationalReader>.
        // This test pins object safety so a future trait change
        // doesn't accidentally break dynamic dispatch.
        let mut r: Box<dyn RelationalReader> = Box::new(users_reader());
        r.open(&ScanContext::default()).await.unwrap();
        let rows = drain_all(&mut *r).await.unwrap();
        assert_eq!(rows.len(), 3);
    }

    // --- Re-open semantics --------------------------------------

    #[tokio::test]
    async fn vec_reader_reopen_resets_cursor() {
        let mut r = users_reader();
        r.open(&ScanContext::default()).await.unwrap();
        let first = drain_all(&mut r).await.unwrap();
        // Re-open with a different projection.
        r.open(&ScanContext {
            projection: Some(vec!["id".into()]),
            ..ScanContext::default()
        })
        .await
        .unwrap();
        let second = drain_all(&mut r).await.unwrap();
        assert_eq!(first.len(), 3);
        assert_eq!(second.len(), 3);
        assert_eq!(second[0].len(), 1); // only id
    }
}
