// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! # Cross-modal source bridge (Track B — the §8 "zero-ETL multimodal" moat)
//!
//! Turns a **vector-search result set** into a DataFusion-joinable `(id, score)`
//! table, so a SINGLE SQL plan can join vector similarity against relational (and,
//! later, graph/document) data over the one canonical `ProximaRecord` spine. This is
//! the substrate of ProximaDB's durable differentiation per
//! `docs/12-design/DATA_WAREHOUSE_AND_ENGINEERING_COURSE_CORRECTION_2026_06_04.adoc`
//! §8: no competitor lets you filter-by-vector-similarity ⋈ relational-aggregate in
//! one query.
//!
//! ## Scope
//! All three non-relational modalities are now joinable relations, live-backed and reachable
//! from the pgwire DataFusion path (registered by `build_session_context`), so a four-way
//! `relational ⋈ vector ⋈ timeseries ⋈ graph` join executes in ONE data-local plan instead of a
//! client-side fan-out + intersect (ADR-053):
//!   * `vector_search(collection, query, k)` → `(id, score)` — via [`VectorSearchTableFunction`].
//!   * `timeseries_range(collection, start_ms, end_ms)` → `(timestamp, metric, value)` — via
//!     [`TimeseriesRangeTableFunction`].
//!   * `graph_traverse(graph_id, start_id, edge_type, max_depth)` → `(node_id, depth)` — via
//!     [`GraphTraverseTableFunction`], over the variable-length reachability traversal.
//!
//! Next slices (ADR-053): a single depth-carrying graph pass (vs the derived-depth `*k..k`
//! passes), and a frontend source node that lowers (via the P4 `logical_lowering`) into the
//! shared logical plane. All reuse the `*_to_batch` bridges below.

use std::collections::HashSet;
use std::sync::Arc;

use arrow_array::{Float32Array, Float64Array, Int32Array, Int64Array, RecordBatch, StringArray};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::{Session, TableFunctionImpl};
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::scalar::ScalarValue;
use proximadb_graph_query::service::GraphQueryReadService;
use proximadb_query::graph_lowering::parse_supported_graph_query;
use proximadb_query::graph_runtime::{
    execute_supported_graph_query_with_start_nodes, graph_query_row_id,
};

use crate::proto::proximadb_v1::{SearchQuery, SearchVectorRecord, VectorSearchRequest};
use crate::services::timeseries_service::{TimeSeriesService, TsPoint};

/// Hard cap on `graph_traverse` depth — bounds the number of `*k..k` traversal passes
/// (each is O(V+E)) regardless of a user-supplied `max_depth`.
const MAX_GRAPH_TRAVERSE_DEPTH: i64 = 64;

/// The lean Arrow schema a vector-search source exposes for joins: `(id, score)`.
/// `id` joins against a relational key; `score` is the similarity the SQL can rank
/// or filter on.
pub fn vector_matches_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("score", DataType::Float32, false),
    ]))
}

/// Convert vector-search results into an `(id, score)` [`RecordBatch`] that DataFusion
/// can register as a table and JOIN against relational data on `id`. This is the
/// bridge from the vector modality into the shared (DataFusion) query plane.
pub fn vector_matches_to_batch(results: &[SearchVectorRecord]) -> Result<RecordBatch, ArrowError> {
    let ids: Vec<&str> = results.iter().map(|r| r.id.as_str()).collect();
    let scores: Vec<f32> = results.iter().map(|r| r.score as f32).collect();
    RecordBatch::try_new(
        vector_matches_schema(),
        vec![
            Arc::new(StringArray::from(ids)),
            Arc::new(Float32Array::from(scores)),
        ],
    )
}

/// A DataFusion [`TableProvider`] whose `scan` runs a **live** vector search through
/// [`VectorOpsPort`] and exposes the `(id, score)` matches as a table — so a single
/// SQL plan can join vector similarity against relational data (§8 moat). This is the
/// production-backed counterpart of [`vector_matches_to_batch`]: register it in a
/// `SessionContext` and the query planner can scan/join/order it like any table.
pub struct VectorSearchTableProvider {
    vector_ops: Arc<dyn proximadb_runtime::VectorOpsPort>,
    collection_id: String,
    query_vector: Vec<f32>,
    top_k: u32,
    tenant_id: Option<String>,
}

impl VectorSearchTableProvider {
    /// Build a provider for one parameterized similarity search.
    pub fn new(
        vector_ops: Arc<dyn proximadb_runtime::VectorOpsPort>,
        collection_id: impl Into<String>,
        query_vector: Vec<f32>,
        top_k: u32,
        tenant_id: Option<String>,
    ) -> Self {
        Self {
            vector_ops,
            collection_id: collection_id.into(),
            query_vector,
            top_k,
            tenant_id,
        }
    }
}

// Manual `Debug` (required by `TableProvider`): the `VectorOpsPort` trait object is
// not `Debug`, so print only the query parameters.
impl std::fmt::Debug for VectorSearchTableProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorSearchTableProvider")
            .field("collection_id", &self.collection_id)
            .field("top_k", &self.top_k)
            .field("tenant_id", &self.tenant_id)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl TableProvider for VectorSearchTableProvider {
    fn schema(&self) -> SchemaRef {
        vector_matches_schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> DFResult<Arc<dyn ExecutionPlan>> {
        // Run the live similarity search and bridge the matches into the plane.
        let request = VectorSearchRequest {
            collection_id: self.collection_id.clone(),
            queries: vec![SearchQuery {
                vector: self.query_vector.clone(),
                ..Default::default()
            }],
            top_k: self.top_k,
            ..Default::default()
        };
        let response = self
            .vector_ops
            .search(request, self.tenant_id.as_deref())
            .await
            .map_err(|e| DataFusionError::Execution(format!("vector search: {e}")))?;
        let results = response.results.map(|sr| sr.results).unwrap_or_default();
        let batch = vector_matches_to_batch(&results).map_err(DataFusionError::from)?;
        // Delegate to a `MemTable` so projection/filter/limit are honored uniformly.
        let mem = MemTable::try_new(vector_matches_schema(), vec![vec![batch]])?;
        mem.scan(state, projection, filters, limit).await
    }
}

/// DataFusion table-valued function `vector_search(collection, query, k)` returning a
/// [`VectorSearchTableProvider`] — makes the cross-modal join expressible directly in
/// SQL (and so reachable from the pgwire DataFusion path):
/// `SELECT d.title, v.score
///  FROM docs d JOIN vector_search('docs_vec', '[0.1,0.2,0.3]', 10) v ON d.id = v.id`.
/// Register once per `SessionContext`:
/// `ctx.register_udtf("vector_search", Arc::new(VectorSearchTableFunction::new(ops)))`.
pub struct VectorSearchTableFunction {
    vector_ops: Arc<dyn proximadb_runtime::VectorOpsPort>,
    /// The connection tenant, forwarded to `VectorOpsPort::search` so the search runs under the
    /// same `TenantContext` the REST write used. `None` ⇒ unscoped (reads only tenant-less data).
    tenant: Option<String>,
}

impl VectorSearchTableFunction {
    /// Capture the live vector service the function will search (no tenant scope).
    pub fn new(vector_ops: Arc<dyn proximadb_runtime::VectorOpsPort>) -> Self {
        Self {
            vector_ops,
            tenant: None,
        }
    }

    /// Capture the vector service AND the connection tenant — so the search reads the same
    /// tenant partition a REST insert wrote. Without this the search runs `tenant_id=None` and
    /// returns 0 rows for tenant-scoped data (TD-XMODAL-6).
    pub fn with_tenant(
        vector_ops: Arc<dyn proximadb_runtime::VectorOpsPort>,
        tenant: Option<String>,
    ) -> Self {
        Self { vector_ops, tenant }
    }
}

impl std::fmt::Debug for VectorSearchTableFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VectorSearchTableFunction")
            .finish_non_exhaustive()
    }
}

impl TableFunctionImpl for VectorSearchTableFunction {
    fn call(&self, args: &[Expr]) -> DFResult<Arc<dyn TableProvider>> {
        let collection = arg_string(args, 0).ok_or_else(|| {
            DataFusionError::Plan(
                "vector_search(collection, query, k): arg 1 must be a collection-name string"
                    .into(),
            )
        })?;
        let query_text = arg_string(args, 1).ok_or_else(|| {
            DataFusionError::Plan(
                "vector_search: arg 2 must be a '[..]' query-vector string".into(),
            )
        })?;
        let top_k = arg_i64(args, 2).ok_or_else(|| {
            DataFusionError::Plan("vector_search: arg 3 must be an integer top_k".into())
        })?;
        let query_vector = parse_vector_literal(&query_text).ok_or_else(|| {
            DataFusionError::Plan(format!(
                "vector_search: cannot parse query vector {query_text:?}"
            ))
        })?;
        if collection.trim().is_empty() {
            return Err(DataFusionError::Plan(
                "vector_search: collection must not be empty".into(),
            ));
        }
        if query_vector.is_empty() {
            return Err(DataFusionError::Plan(
                "vector_search: query vector must contain at least one dimension".into(),
            ));
        }
        if top_k <= 0 {
            return Err(DataFusionError::Plan(
                "vector_search: top_k must be greater than zero".into(),
            ));
        }
        Ok(Arc::new(VectorSearchTableProvider::new(
            self.vector_ops.clone(),
            collection,
            query_vector,
            top_k as u32,
            self.tenant.clone(),
        )))
    }
}

/// Extract a string-literal argument at position `i`.
fn arg_string(args: &[Expr], i: usize) -> Option<String> {
    match args.get(i)? {
        Expr::Literal(ScalarValue::Utf8(Some(s)), _)
        | Expr::Literal(ScalarValue::LargeUtf8(Some(s)), _) => Some(s.clone()),
        _ => None,
    }
}

/// Extract an integer-literal argument at position `i`.
fn arg_i64(args: &[Expr], i: usize) -> Option<i64> {
    match args.get(i)? {
        Expr::Literal(ScalarValue::Int64(Some(n)), _) => Some(*n),
        Expr::Literal(ScalarValue::Int32(Some(n)), _) => Some(*n as i64),
        Expr::Literal(ScalarValue::UInt64(Some(n)), _) => Some(*n as i64),
        _ => None,
    }
}

/// Fold the connection tenant into the collection key exactly as the REST write path's
/// `effective_collection` (`src/network/rest/v2/timeseries.rs`) does — `{tenant}::{collection}` —
/// so a UDTF read hits the same partition a REST ingest wrote (TD-XMODAL-6). An empty/absent
/// tenant yields the raw name, mirroring the REST rule so single-tenant reads still resolve.
/// (Vectors are scoped by `TenantContext`, not by name, so they use the raw collection + the
/// tenant id directly; this fold is the timeseries analogue.)
fn tenant_scoped_collection(tenant: Option<&str>, collection: &str) -> String {
    match tenant {
        Some(t) if !t.is_empty() => format!("{t}::{collection}"),
        _ => collection.to_string(),
    }
}

/// Parse a pgvector-style text literal `[0.1, 0.2, 0.3]` into `Vec<f32>`.
fn parse_vector_literal(text: &str) -> Option<Vec<f32>> {
    let inner = text.trim().strip_prefix('[')?.strip_suffix(']')?;
    if inner.trim().is_empty() {
        return Some(Vec::new());
    }
    inner
        .split(',')
        .map(|p| p.trim().parse::<f32>().ok())
        .collect()
}

// ─────────────────────────────────────────────────────────────────────────────
// Timeseries slice (§8 moat, second modality) — `timeseries_range(collection, start_ms,
// end_ms)` as a joinable table. The timeseries analogue of `vector_search`: it turns a
// time-range read into a DataFusion `(timestamp, metric, value)` relation so a SINGLE SQL
// plan can join a timeseries anomaly-scan against relational/graph data — the graph⋈timeseries
// root-cause join the vertical copilots do client-side today becomes one data-local query.
// ─────────────────────────────────────────────────────────────────────────────

/// The Arrow schema a timeseries-range source exposes for joins: `(timestamp, metric, value)`
/// in long form — one row per (point, value-column), so a collection with several value
/// columns projects generically and SQL can `WHERE metric = '…'` + bucket/aggregate.
pub fn timeseries_range_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("timestamp", DataType::Int64, false),
        Field::new("metric", DataType::Utf8, false),
        Field::new("value", DataType::Float64, false),
    ]))
}

/// Convert time-series points into a `(timestamp, metric, value)` [`RecordBatch`]. Value columns
/// are emitted in sorted name order for deterministic output.
pub fn timeseries_points_to_batch(points: &[TsPoint]) -> Result<RecordBatch, ArrowError> {
    let mut timestamps: Vec<i64> = Vec::new();
    let mut metrics: Vec<String> = Vec::new();
    let mut values: Vec<f64> = Vec::new();
    for point in points {
        let mut cols: Vec<(&String, &f64)> = point.values.iter().collect();
        cols.sort_by(|a, b| a.0.cmp(b.0));
        for (metric, value) in cols {
            timestamps.push(point.timestamp);
            metrics.push(metric.clone());
            values.push(*value);
        }
    }
    RecordBatch::try_new(
        timeseries_range_schema(),
        vec![
            Arc::new(Int64Array::from(timestamps)),
            Arc::new(StringArray::from(metrics)),
            Arc::new(Float64Array::from(values)),
        ],
    )
}

/// Port yielding time-series points for a `[start_ms, end_ms]` range of one collection — the
/// timeseries analogue of `VectorOpsPort`. Kept as a trait so the table source is unit-testable
/// with a fixed set of points, independent of the process time-series service.
#[async_trait]
pub trait TimeseriesScanPort: Send + Sync {
    /// Read the points of `collection` in `[start_ms, end_ms]` (epoch millis).
    async fn range(
        &self,
        collection: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> anyhow::Result<Vec<TsPoint>>;
}

/// Production adapter: the process [`TimeSeriesService`] as a [`TimeseriesScanPort`].
struct TimeSeriesServiceScan(Arc<TimeSeriesService>);

#[async_trait]
impl TimeseriesScanPort for TimeSeriesServiceScan {
    async fn range(
        &self,
        collection: &str,
        start_ms: i64,
        end_ms: i64,
    ) -> anyhow::Result<Vec<TsPoint>> {
        self.0.query(collection, start_ms, end_ms, None).await
    }
}

/// A DataFusion [`TableProvider`] whose `scan` reads a live time range through a
/// [`TimeseriesScanPort`] and exposes `(timestamp, metric, value)` rows — so a single SQL plan
/// can join a timeseries scan against relational/graph data.
pub struct TimeseriesRangeTableProvider {
    scan: Arc<dyn TimeseriesScanPort>,
    collection: String,
    start_ms: i64,
    end_ms: i64,
}

impl TimeseriesRangeTableProvider {
    /// Build a provider for one parameterized time-range read.
    pub fn new(
        scan: Arc<dyn TimeseriesScanPort>,
        collection: impl Into<String>,
        start_ms: i64,
        end_ms: i64,
    ) -> Self {
        Self {
            scan,
            collection: collection.into(),
            start_ms,
            end_ms,
        }
    }
}

impl std::fmt::Debug for TimeseriesRangeTableProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeseriesRangeTableProvider")
            .field("collection", &self.collection)
            .field("start_ms", &self.start_ms)
            .field("end_ms", &self.end_ms)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl TableProvider for TimeseriesRangeTableProvider {
    fn schema(&self) -> SchemaRef {
        timeseries_range_schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> DFResult<Arc<dyn ExecutionPlan>> {
        let points = self
            .scan
            .range(&self.collection, self.start_ms, self.end_ms)
            .await
            .map_err(|e| DataFusionError::Execution(format!("timeseries range: {e}")))?;
        let batch = timeseries_points_to_batch(&points).map_err(DataFusionError::from)?;
        // Delegate to a `MemTable` so projection/filter/limit are honored uniformly.
        let mem = MemTable::try_new(timeseries_range_schema(), vec![vec![batch]])?;
        mem.scan(state, projection, filters, limit).await
    }
}

/// DataFusion table-valued function `timeseries_range(collection, start_ms, end_ms)` returning a
/// [`TimeseriesRangeTableProvider`] — makes the timeseries⋈relational/graph join expressible
/// directly in SQL (and so reachable from the pgwire DataFusion path):
/// `SELECT t.metric, MAX(t.value)
///  FROM timeseries_range('sensor', 1700000000000, 1700003600000) t GROUP BY t.metric`.
/// Register once per `SessionContext`:
/// `ctx.register_udtf("timeseries_range", Arc::new(TimeseriesRangeTableFunction::new(port)))`.
pub struct TimeseriesRangeTableFunction {
    scan: Arc<dyn TimeseriesScanPort>,
    /// The connection tenant, folded into the collection key (`{tenant}::{collection}`) at
    /// `call` time so the range read hits the same partition a REST ingest wrote. `None` ⇒ raw
    /// collection name (single-tenant / tenant-less data).
    tenant: Option<String>,
}

impl TimeseriesRangeTableFunction {
    /// Capture the scan port the function will read (no tenant scope).
    pub fn new(scan: Arc<dyn TimeseriesScanPort>) -> Self {
        Self { scan, tenant: None }
    }

    /// Capture the scan port AND the connection tenant, so the range read folds the collection
    /// key the same way the REST write path does (TD-XMODAL-6).
    pub fn with_tenant(scan: Arc<dyn TimeseriesScanPort>, tenant: Option<String>) -> Self {
        Self { scan, tenant }
    }

    /// Build the function backed by the process time-series service (no tenant scope).
    pub fn from_service(service: Arc<TimeSeriesService>) -> Self {
        Self {
            scan: Arc::new(TimeSeriesServiceScan(service)),
            tenant: None,
        }
    }

    /// Build the function backed by the process time-series service, scoped to the connection
    /// tenant — the pgwire OLAP route wiring that makes `timeseries_range` read REST-written data.
    pub fn from_service_with_tenant(
        service: Arc<TimeSeriesService>,
        tenant: Option<String>,
    ) -> Self {
        Self {
            scan: Arc::new(TimeSeriesServiceScan(service)),
            tenant,
        }
    }
}

impl std::fmt::Debug for TimeseriesRangeTableFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TimeseriesRangeTableFunction")
            .finish_non_exhaustive()
    }
}

impl TableFunctionImpl for TimeseriesRangeTableFunction {
    fn call(&self, args: &[Expr]) -> DFResult<Arc<dyn TableProvider>> {
        let collection = arg_string(args, 0).ok_or_else(|| {
            DataFusionError::Plan(
                "timeseries_range(collection, start_ms, end_ms): arg 1 must be a collection-name string"
                    .into(),
            )
        })?;
        let start_ms = arg_i64(args, 1).ok_or_else(|| {
            DataFusionError::Plan("timeseries_range: arg 2 must be an integer start_ms".into())
        })?;
        let end_ms = arg_i64(args, 2).ok_or_else(|| {
            DataFusionError::Plan("timeseries_range: arg 3 must be an integer end_ms".into())
        })?;
        if collection.trim().is_empty() {
            return Err(DataFusionError::Plan(
                "timeseries_range: collection must not be empty".into(),
            ));
        }
        if end_ms < start_ms {
            return Err(DataFusionError::Plan(
                "timeseries_range: end_ms must be >= start_ms".into(),
            ));
        }
        // Fold the tenant into the collection key the same way the REST write path does, so the
        // range read resolves the partition the ingest wrote (TD-XMODAL-6).
        let collection = tenant_scoped_collection(self.tenant.as_deref(), &collection);
        Ok(Arc::new(TimeseriesRangeTableProvider::new(
            self.scan.clone(),
            collection,
            start_ms,
            end_ms,
        )))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Graph slice (§8 moat, third modality) — `graph_traverse(graph_id, start_id, edge_type,
// max_depth)` as a joinable table. The graph analogue of vector_search / timeseries_range:
// it turns a variable-length reachability traversal into a DataFusion `(node_id, depth)`
// relation, so a SINGLE SQL plan can join graph reachability against relational / vector /
// timeseries data — the four-way zero-ETL cross-modal join over the pgwire OLAP route.
// ─────────────────────────────────────────────────────────────────────────────

/// The Arrow schema a graph-traversal source exposes for joins: `(node_id, depth)`.
pub fn graph_traverse_schema() -> SchemaRef {
    Arc::new(Schema::new(vec![
        Field::new("node_id", DataType::Utf8, false),
        Field::new("depth", DataType::Int32, false),
    ]))
}

/// Convert `(node_id, depth)` reachable-node rows into a [`RecordBatch`].
pub fn graph_nodes_to_batch(nodes: &[(String, i32)]) -> Result<RecordBatch, ArrowError> {
    let ids: Vec<&str> = nodes.iter().map(|(id, _)| id.as_str()).collect();
    let depths: Vec<i32> = nodes.iter().map(|(_, d)| *d).collect();
    RecordBatch::try_new(
        graph_traverse_schema(),
        vec![
            Arc::new(StringArray::from(ids)),
            Arc::new(Int32Array::from(depths)),
        ],
    )
}

/// A DataFusion [`TableProvider`] whose `scan` walks a graph's variable-length reachability
/// (`start_id -[:edge_type*1..max_depth]-> x`) live through a [`GraphQueryReadService`] and
/// exposes the reachable `(node_id, depth)` set — so a single SQL plan can join graph
/// reachability against relational / vector / timeseries data.
pub struct GraphTraverseTableProvider {
    graph_ops: Arc<dyn GraphQueryReadService>,
    graph_id: String,
    start_id: String,
    edge_type: String,
    max_depth: i64,
}

impl GraphTraverseTableProvider {
    /// Build a provider for one parameterized reachability traversal.
    pub fn new(
        graph_ops: Arc<dyn GraphQueryReadService>,
        graph_id: impl Into<String>,
        start_id: impl Into<String>,
        edge_type: impl Into<String>,
        max_depth: i64,
    ) -> Self {
        Self {
            graph_ops,
            graph_id: graph_id.into(),
            start_id: start_id.into(),
            edge_type: edge_type.into(),
            max_depth,
        }
    }
}

impl std::fmt::Debug for GraphTraverseTableProvider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphTraverseTableProvider")
            .field("graph_id", &self.graph_id)
            .field("start_id", &self.start_id)
            .field("edge_type", &self.edge_type)
            .field("max_depth", &self.max_depth)
            .finish_non_exhaustive()
    }
}

#[async_trait]
impl TableProvider for GraphTraverseTableProvider {
    fn schema(&self) -> SchemaRef {
        graph_traverse_schema()
    }

    fn table_type(&self) -> TableType {
        TableType::Base
    }

    async fn scan(
        &self,
        state: &dyn Session,
        projection: Option<&Vec<usize>>,
        filters: &[Expr],
        limit: Option<usize>,
    ) -> DFResult<Arc<dyn ExecutionPlan>> {
        // Derived-depth traversal: one `*k..k` reachability pass per depth k in 1..=max, tagging
        // each first-seen node with its (shortest) depth. The inner BFS is visited-set bounded,
        // so each pass is O(V+E); a single depth-carrying pass is a follow-on TD.
        let max_depth = self.max_depth.clamp(1, MAX_GRAPH_TRAVERSE_DEPTH);
        let mut rows: Vec<(String, i32)> = Vec::new();
        let mut seen: HashSet<String> = HashSet::new();
        for k in 1..=max_depth {
            let cypher = format!(
                "MATCH (a)-[:{edge}*{k}..{k}]->(x) RETURN x.id AS node_id",
                edge = self.edge_type
            );
            let parsed = parse_supported_graph_query(&cypher, None, Some(&self.graph_id))
                .map_err(|e| DataFusionError::Execution(format!("graph_traverse parse: {e}")))?;
            let executed = execute_supported_graph_query_with_start_nodes(
                &*self.graph_ops,
                &parsed,
                Some(std::slice::from_ref(&self.start_id)),
            )
            .await
            .map_err(|e| DataFusionError::Execution(format!("graph_traverse: {e}")))?;
            for row in &executed.rows {
                let id = graph_query_row_id(row, 0);
                if !id.is_empty() && seen.insert(id.clone()) {
                    rows.push((id, k as i32));
                }
            }
        }
        let batch = graph_nodes_to_batch(&rows).map_err(DataFusionError::from)?;
        let mem = MemTable::try_new(graph_traverse_schema(), vec![vec![batch]])?;
        mem.scan(state, projection, filters, limit).await
    }
}

/// DataFusion table-valued function `graph_traverse(graph_id, start_id, edge_type, max_depth)`
/// returning a [`GraphTraverseTableProvider`] — makes the reachability ⋈ relational join
/// expressible directly in SQL (reachable from the pgwire DataFusion path):
/// `SELECT a.tier, g.node_id, g.depth
///  FROM accounts a JOIN graph_traverse('aml','acct-7','sent_to',4) g ON a.id = g.node_id`.
/// Register once per `SessionContext`:
/// `ctx.register_udtf("graph_traverse", Arc::new(GraphTraverseTableFunction::new(graph_ops)))`.
pub struct GraphTraverseTableFunction {
    graph_ops: Arc<dyn GraphQueryReadService>,
}

impl GraphTraverseTableFunction {
    /// Capture the live graph read service the function will traverse.
    ///
    /// Unlike `vector_search` / `timeseries_range`, this takes NO tenant: the graph write path
    /// (`GraphPort`, the v1/v2 REST graph handlers) applies no tenant scoping today — the
    /// `graph_id` is the sole scope key on both sides — so a UDTF read of the raw `graph_id`
    /// already matches the write. Real graph tenant isolation (a tenant param on `GraphPort`,
    /// applied on both write and this read) is a follow-up (see TD-XMODAL-6).
    pub fn new(graph_ops: Arc<dyn GraphQueryReadService>) -> Self {
        Self { graph_ops }
    }
}

impl std::fmt::Debug for GraphTraverseTableFunction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GraphTraverseTableFunction")
            .finish_non_exhaustive()
    }
}

/// A bare identifier (edge type / relationship name) safe to interpolate into the internal
/// Cypher: non-empty ASCII alphanumerics + `_`. (The start-node id is passed out-of-band via
/// `execute_supported_graph_query_with_start_nodes`, so it never enters the query string.)
fn is_bare_identifier(s: &str) -> bool {
    !s.is_empty() && s.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
}

impl TableFunctionImpl for GraphTraverseTableFunction {
    fn call(&self, args: &[Expr]) -> DFResult<Arc<dyn TableProvider>> {
        let graph_id = arg_string(args, 0).ok_or_else(|| {
            DataFusionError::Plan(
                "graph_traverse(graph_id, start_id, edge_type, max_depth): arg 1 must be a graph-id string"
                    .into(),
            )
        })?;
        let start_id = arg_string(args, 1).ok_or_else(|| {
            DataFusionError::Plan("graph_traverse: arg 2 must be a start-node-id string".into())
        })?;
        let edge_type = arg_string(args, 2).ok_or_else(|| {
            DataFusionError::Plan("graph_traverse: arg 3 must be an edge-type string".into())
        })?;
        let max_depth = arg_i64(args, 3).ok_or_else(|| {
            DataFusionError::Plan("graph_traverse: arg 4 must be an integer max_depth".into())
        })?;
        if graph_id.trim().is_empty() {
            return Err(DataFusionError::Plan(
                "graph_traverse: graph_id must not be empty".into(),
            ));
        }
        if start_id.trim().is_empty() {
            return Err(DataFusionError::Plan(
                "graph_traverse: start_id must not be empty".into(),
            ));
        }
        if !is_bare_identifier(&edge_type) {
            return Err(DataFusionError::Plan(
                "graph_traverse: edge_type must be a bare identifier (letters, digits, underscore)"
                    .into(),
            ));
        }
        if max_depth <= 0 {
            return Err(DataFusionError::Plan(
                "graph_traverse: max_depth must be greater than zero".into(),
            ));
        }
        Ok(Arc::new(GraphTraverseTableProvider::new(
            self.graph_ops.clone(),
            graph_id,
            start_id,
            edge_type,
            max_depth,
        )))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use datafusion::datasource::MemTable;
    use datafusion::prelude::SessionContext;

    fn sv(id: &str, score: f64) -> SearchVectorRecord {
        SearchVectorRecord {
            id: id.to_string(),
            score,
            ..Default::default()
        }
    }

    #[test]
    fn vector_matches_batch_has_id_score_schema() {
        let batch = vector_matches_to_batch(&[sv("a", 0.9), sv("b", 0.5)]).unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.schema().field(0).name(), "id");
        assert_eq!(batch.schema().field(1).name(), "score");
    }

    /// The moat proof: vector-search results JOIN a relational table in ONE
    /// DataFusion SQL plan (filter-by-similarity ⋈ relational), ordered by score.
    #[tokio::test]
    async fn vector_matches_join_relational_in_one_sql_plan() {
        let ctx = SessionContext::new();

        // Vector modality → joinable table (would come from the live VectorOpsPort
        // in the next slice; here we feed a fixed result set through the bridge).
        let matches = vector_matches_to_batch(&[sv("a", 0.95), sv("b", 0.80), sv("c", 0.70)])
            .expect("matches batch");
        ctx.register_table(
            "vmatches",
            Arc::new(MemTable::try_new(vector_matches_schema(), vec![vec![matches]]).unwrap()),
        )
        .unwrap();

        // Relational modality.
        let docs_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
        ]));
        let docs = RecordBatch::try_new(
            docs_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "z"])),
                Arc::new(StringArray::from(vec!["Alpha", "Bravo", "Zulu"])),
            ],
        )
        .unwrap();
        ctx.register_table(
            "docs",
            Arc::new(MemTable::try_new(docs_schema, vec![vec![docs]]).unwrap()),
        )
        .unwrap();

        // One SQL plan joining vector similarity with relational rows.
        let df = ctx
            .sql(
                "SELECT d.id, d.title, m.score \
                 FROM docs d JOIN vmatches m ON d.id = m.id \
                 ORDER BY m.score DESC",
            )
            .await
            .unwrap();
        let batches = df.collect().await.unwrap();
        let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        // a(Alpha,0.95) + b(Bravo,0.80); c has no doc, z has no vector match.
        assert_eq!(rows, 2);
    }

    use crate::proto::proximadb_v1::{SearchResult, VectorBatchRequest, VectorOperationResponse};

    /// A fixed `VectorOpsPort` that returns a canned similarity result — stands in for
    /// the live vector service so the provider's `scan` path is exercised.
    struct FixedVectorOps {
        matches: Vec<(String, f64)>,
    }

    #[async_trait]
    impl proximadb_runtime::VectorOpsPort for FixedVectorOps {
        async fn search(
            &self,
            _request: VectorSearchRequest,
            _tenant_id: Option<&str>,
        ) -> anyhow::Result<VectorOperationResponse> {
            let results = self
                .matches
                .iter()
                .map(|(id, score)| SearchVectorRecord {
                    id: id.clone(),
                    score: *score,
                    ..Default::default()
                })
                .collect();
            Ok(VectorOperationResponse {
                results: Some(SearchResult {
                    results,
                    ..Default::default()
                }),
                ..Default::default()
            })
        }
        async fn batch_upsert(
            &self,
            _r: VectorBatchRequest,
            _t: Option<&str>,
        ) -> anyhow::Result<VectorOperationResponse> {
            unimplemented!()
        }
        async fn get_vector(
            &self,
            _c: &str,
            _v: &str,
            _iv: bool,
            _im: bool,
            _t: Option<&str>,
        ) -> anyhow::Result<VectorOperationResponse> {
            unimplemented!()
        }
        async fn flush_all(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn metrics(&self) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
    }

    /// The live-backed moat: a `VectorSearchTableProvider` (running a search through a
    /// `VectorOpsPort`) registered as a table, scanned AND joined with relational data
    /// in one SQL plan.
    #[tokio::test]
    async fn vector_search_provider_scans_and_joins() {
        let ops: Arc<dyn proximadb_runtime::VectorOpsPort> = Arc::new(FixedVectorOps {
            matches: vec![("a".into(), 0.95), ("b".into(), 0.80), ("c".into(), 0.70)],
        });
        let provider =
            VectorSearchTableProvider::new(ops, "docs_vec", vec![0.1, 0.2, 0.3], 10, None);

        let ctx = SessionContext::new();
        ctx.register_table("vsearch", Arc::new(provider)).unwrap();

        // (1) Scan the live-backed provider directly.
        let scanned: usize = ctx
            .sql("SELECT id, score FROM vsearch ORDER BY score DESC")
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
            .iter()
            .map(|b| b.num_rows())
            .sum();
        assert_eq!(scanned, 3);

        // (2) Join it with a relational table in ONE plan.
        let docs_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
        ]));
        let docs = RecordBatch::try_new(
            docs_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "z"])),
                Arc::new(StringArray::from(vec!["Alpha", "Bravo", "Zulu"])),
            ],
        )
        .unwrap();
        ctx.register_table(
            "docs",
            Arc::new(MemTable::try_new(docs_schema, vec![vec![docs]]).unwrap()),
        )
        .unwrap();
        let joined: usize = ctx
            .sql(
                "SELECT d.title, v.score FROM docs d JOIN vsearch v ON d.id = v.id \
                 ORDER BY v.score DESC",
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
            .iter()
            .map(|b| b.num_rows())
            .sum();
        assert_eq!(joined, 2); // a + b match docs; c has no doc, z has no match.
    }

    #[test]
    fn parses_pgvector_text_literal() {
        assert_eq!(
            parse_vector_literal("[0.1, 0.2, 0.3]"),
            Some(vec![0.1, 0.2, 0.3])
        );
        assert_eq!(parse_vector_literal("[]"), Some(vec![]));
        assert_eq!(parse_vector_literal("0.1,0.2"), None); // missing brackets
    }

    /// The customer-facing moat: a single SQL statement (via the `vector_search` UDTF)
    /// joins vector similarity with relational data — the shape a pgwire client writes.
    #[tokio::test]
    async fn vector_search_udtf_joins_relational_in_sql() {
        let ops: Arc<dyn proximadb_runtime::VectorOpsPort> = Arc::new(FixedVectorOps {
            matches: vec![("a".into(), 0.95), ("b".into(), 0.80), ("c".into(), 0.70)],
        });
        let ctx = SessionContext::new();
        ctx.register_udtf(
            "vector_search",
            Arc::new(VectorSearchTableFunction::new(ops)),
        );

        let docs_schema = Arc::new(Schema::new(vec![
            Field::new("id", DataType::Utf8, false),
            Field::new("title", DataType::Utf8, false),
        ]));
        let docs = RecordBatch::try_new(
            docs_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["a", "b", "z"])),
                Arc::new(StringArray::from(vec!["Alpha", "Bravo", "Zulu"])),
            ],
        )
        .unwrap();
        ctx.register_table(
            "docs",
            Arc::new(MemTable::try_new(docs_schema, vec![vec![docs]]).unwrap()),
        )
        .unwrap();

        let n: usize = ctx
            .sql(
                "SELECT d.title, v.score \
                 FROM docs d JOIN vector_search('docs_vec', '[0.1,0.2,0.3]', 10) v \
                   ON d.id = v.id \
                 ORDER BY v.score DESC",
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
            .iter()
            .map(|b| b.num_rows())
            .sum();
        assert_eq!(n, 2); // a + b
    }

    #[tokio::test]
    async fn vector_search_udtf_rejects_invalid_inputs() {
        let ops: Arc<dyn proximadb_runtime::VectorOpsPort> =
            Arc::new(FixedVectorOps { matches: vec![] });
        let ctx = SessionContext::new();
        ctx.register_udtf(
            "vector_search",
            Arc::new(VectorSearchTableFunction::new(ops)),
        );

        for (sql, expected) in [
            (
                "SELECT * FROM vector_search('', '[0.1,0.2]', 10)",
                "collection must not be empty",
            ),
            (
                "SELECT * FROM vector_search('docs_vec', '[]', 10)",
                "query vector must contain at least one dimension",
            ),
            (
                "SELECT * FROM vector_search('docs_vec', '[0.1,0.2]', 0)",
                "top_k must be greater than zero",
            ),
        ] {
            let error = ctx.sql(sql).await.expect_err("invalid vector_search");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error}"
            );
        }
    }

    #[tokio::test]
    async fn live_session_context_registers_vector_search() {
        // F4: the live session-context builder registers `vector_search` itself, so the
        // cross-modal table function is available over the DataFusion path WITHOUT a manual
        // register_udtf — this is exactly how the pgwire OLAP route wires it.
        let ops: Arc<dyn proximadb_runtime::VectorOpsPort> = Arc::new(FixedVectorOps {
            matches: vec![("a".into(), 0.95), ("b".into(), 0.80)],
        });
        let ctx = crate::datafusion::create_session_context_with_vector_ops(ops).unwrap();
        let n: usize = ctx
            .sql("SELECT id, score FROM vector_search('docs_vec', '[0.1,0.2]', 10)")
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
            .iter()
            .map(|b| b.num_rows())
            .sum();
        assert_eq!(n, 2);
    }

    // ── timeseries slice ────────────────────────────────────────────────────────
    use std::collections::HashMap;

    fn tp(timestamp: i64, metric: &str, value: f64) -> TsPoint {
        TsPoint {
            timestamp,
            values: HashMap::from([(metric.to_string(), value)]),
            tags: HashMap::new(),
        }
    }

    /// A fixed `TimeseriesScanPort` returning canned points — stands in for the process
    /// time-series service so the provider's `scan` path is exercised without engine state.
    struct FixedTimeseriesScan {
        points: Vec<TsPoint>,
    }

    #[async_trait]
    impl TimeseriesScanPort for FixedTimeseriesScan {
        async fn range(
            &self,
            _collection: &str,
            _start_ms: i64,
            _end_ms: i64,
        ) -> anyhow::Result<Vec<TsPoint>> {
            Ok(self.points.clone())
        }
    }

    #[test]
    fn timeseries_batch_has_timestamp_metric_value_schema() {
        let batch =
            timeseries_points_to_batch(&[tp(10, "amount", 1.5), tp(20, "amount", 2.5)]).unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.schema().field(0).name(), "timestamp");
        assert_eq!(batch.schema().field(1).name(), "metric");
        assert_eq!(batch.schema().field(2).name(), "value");
    }

    /// The moat proof for the second modality: a timeseries scan JOINs a relational table in
    /// ONE DataFusion SQL plan — the shape a pgwire client (or a copilot) writes instead of
    /// fanning out N per-entity reads and intersecting them client-side.
    #[tokio::test]
    async fn timeseries_range_udtf_joins_relational_in_sql() {
        let port: Arc<dyn TimeseriesScanPort> = Arc::new(FixedTimeseriesScan {
            points: vec![
                tp(1_000, "amount", 100.0),
                tp(2_000, "amount", 9_000.0),
                tp(3_000, "amount", 50.0),
            ],
        });
        let ctx = SessionContext::new();
        ctx.register_udtf(
            "timeseries_range",
            Arc::new(TimeseriesRangeTableFunction::new(port)),
        );

        // Relational side: an entity dimension the timeseries joins onto by metric name.
        let dim_schema = Arc::new(Schema::new(vec![
            Field::new("metric", DataType::Utf8, false),
            Field::new("label", DataType::Utf8, false),
        ]));
        let dim = RecordBatch::try_new(
            dim_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["amount"])),
                Arc::new(StringArray::from(vec!["claimed"])),
            ],
        )
        .unwrap();
        ctx.register_table(
            "metrics",
            Arc::new(MemTable::try_new(dim_schema, vec![vec![dim]]).unwrap()),
        )
        .unwrap();

        // One SQL plan: the timeseries anomaly-scan (MAX per metric) ⋈ the relational dimension.
        let batches = ctx
            .sql(
                "SELECT m.label, MAX(t.value) AS peak \
                 FROM timeseries_range('ts_clmt_007', 0, 9999999) t \
                 JOIN metrics m ON t.metric = m.metric \
                 GROUP BY m.label",
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        let rows: usize = batches.iter().map(|b| b.num_rows()).sum();
        assert_eq!(rows, 1); // one metric group: 'claimed' with peak 9000
        let peak = batches[0]
            .column(1)
            .as_any()
            .downcast_ref::<Float64Array>()
            .unwrap()
            .value(0);
        assert_eq!(peak, 9_000.0);
    }

    #[tokio::test]
    async fn timeseries_range_udtf_rejects_invalid_inputs() {
        let port: Arc<dyn TimeseriesScanPort> = Arc::new(FixedTimeseriesScan { points: vec![] });
        let ctx = SessionContext::new();
        ctx.register_udtf(
            "timeseries_range",
            Arc::new(TimeseriesRangeTableFunction::new(port)),
        );

        for (sql, expected) in [
            (
                "SELECT * FROM timeseries_range('', 0, 10)",
                "collection must not be empty",
            ),
            (
                "SELECT * FROM timeseries_range('ts', 10, 0)",
                "end_ms must be >= start_ms",
            ),
        ] {
            let error = ctx.sql(sql).await.expect_err("invalid timeseries_range");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error}"
            );
        }
    }

    /// TD-XMODAL-7 repro: a UDTF wrapped in an aggregating derived table that is then JOINed with
    /// a base table (`FROM base o CROSS JOIN (SELECT max(value) FROM timeseries_range(...)) ts`)
    /// must not blow the stack. A top-level UDTF (even two joined) plans fine; only the
    /// join-to-a-derived-table-over-a-UDTF shape recursed over pgwire's DataFusion fallback.
    #[tokio::test]
    async fn udtf_in_aggregating_subquery_does_not_recurse() {
        let port: Arc<dyn TimeseriesScanPort> = Arc::new(FixedTimeseriesScan {
            points: vec![tp(1, "amount", 5.0), tp(2, "amount", 9.0)],
        });
        let ctx = SessionContext::new();
        ctx.register_udtf(
            "timeseries_range",
            Arc::new(TimeseriesRangeTableFunction::new(port)),
        );
        // A base relation to join against, mirroring the pgwire `orders` table.
        let base_schema = Arc::new(Schema::new(vec![Field::new("id", DataType::Utf8, false)]));
        let base = RecordBatch::try_new(
            base_schema.clone(),
            vec![Arc::new(StringArray::from(vec!["acct-7"]))],
        )
        .unwrap();
        ctx.register_table(
            "orders",
            Arc::new(MemTable::try_new(base_schema, vec![vec![base]]).unwrap()),
        )
        .unwrap();
        let n: usize = ctx
            .sql(
                "SELECT o.id, ts.peak \
                 FROM orders o \
                 CROSS JOIN (SELECT max(value) AS peak FROM timeseries_range('c', 0, 9)) ts \
                 WHERE o.id = 'acct-7' \
                 GROUP BY o.id, ts.peak",
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
            .iter()
            .map(|b| b.num_rows())
            .sum();
        assert_eq!(n, 1);
    }

    /// A `TimeseriesScanPort` that records the collection name it was asked to read — used to
    /// prove `timeseries_range` folds the tenant into the collection key (TD-XMODAL-6).
    struct RecordingTimeseriesScan {
        seen: Arc<std::sync::Mutex<Vec<String>>>,
    }

    #[async_trait]
    impl TimeseriesScanPort for RecordingTimeseriesScan {
        async fn range(
            &self,
            collection: &str,
            _start_ms: i64,
            _end_ms: i64,
        ) -> anyhow::Result<Vec<TsPoint>> {
            self.seen.lock().unwrap().push(collection.to_string());
            Ok(vec![tp(1_000, "amount", 1.0)])
        }
    }

    /// TD-XMODAL-6: a tenant-scoped `timeseries_range` folds `{tenant}::{collection}` before the
    /// read — matching the REST write path's `effective_collection` — so it hits the partition a
    /// REST ingest wrote; an empty/absent tenant reads the raw name.
    #[tokio::test]
    async fn timeseries_range_udtf_folds_tenant_into_collection() {
        for (tenant, expected) in [
            (Some("acme".to_string()), "acme::sensor"),
            (Some(String::new()), "sensor"),
            (None, "sensor"),
        ] {
            let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
            let port: Arc<dyn TimeseriesScanPort> = Arc::new(RecordingTimeseriesScan {
                seen: Arc::clone(&seen),
            });
            let ctx = SessionContext::new();
            ctx.register_udtf(
                "timeseries_range",
                Arc::new(TimeseriesRangeTableFunction::with_tenant(port, tenant)),
            );
            let _ = ctx
                .sql("SELECT * FROM timeseries_range('sensor', 0, 9999)")
                .await
                .unwrap()
                .collect()
                .await
                .unwrap();
            assert_eq!(seen.lock().unwrap().as_slice(), &[expected.to_string()]);
        }
    }

    /// A `VectorOpsPort` that records the `tenant_id` its `search` was called with — proves
    /// `vector_search` forwards the connection tenant (TD-XMODAL-6).
    struct RecordingVectorOps {
        seen: Arc<std::sync::Mutex<Vec<Option<String>>>>,
    }

    #[async_trait]
    impl proximadb_runtime::VectorOpsPort for RecordingVectorOps {
        async fn search(
            &self,
            _request: VectorSearchRequest,
            tenant_id: Option<&str>,
        ) -> anyhow::Result<VectorOperationResponse> {
            self.seen
                .lock()
                .unwrap()
                .push(tenant_id.map(str::to_string));
            Ok(VectorOperationResponse {
                results: Some(SearchResult {
                    results: vec![],
                    ..Default::default()
                }),
                ..Default::default()
            })
        }
        async fn batch_upsert(
            &self,
            _r: VectorBatchRequest,
            _t: Option<&str>,
        ) -> anyhow::Result<VectorOperationResponse> {
            unimplemented!()
        }
        async fn get_vector(
            &self,
            _c: &str,
            _v: &str,
            _iv: bool,
            _im: bool,
            _t: Option<&str>,
        ) -> anyhow::Result<VectorOperationResponse> {
            unimplemented!()
        }
        async fn flush_all(&self) -> anyhow::Result<()> {
            Ok(())
        }
        async fn metrics(&self) -> anyhow::Result<serde_json::Value> {
            Ok(serde_json::Value::Null)
        }
    }

    /// TD-XMODAL-6: a tenant-scoped `vector_search` forwards the connection tenant to the search
    /// (vectors are `TenantContext`-partitioned, not name-folded), so it reads the tenant's data
    /// instead of the unscoped partition.
    #[tokio::test]
    async fn vector_search_udtf_forwards_tenant_to_search() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let ops: Arc<dyn proximadb_runtime::VectorOpsPort> = Arc::new(RecordingVectorOps {
            seen: Arc::clone(&seen),
        });
        let ctx = SessionContext::new();
        ctx.register_udtf(
            "vector_search",
            Arc::new(VectorSearchTableFunction::with_tenant(
                ops,
                Some("acme".to_string()),
            )),
        );
        let _ = ctx
            .sql("SELECT * FROM vector_search('docs_vec', '[0.1,0.2]', 5)")
            .await
            .unwrap()
            .collect()
            .await
            .unwrap();
        assert_eq!(seen.lock().unwrap().as_slice(), &[Some("acme".to_string())]);
    }

    // ── graph slice ───────────────────────────────────────────────────────────────
    use crate::proto::proximadb_v1::{Edge, EdgeQuery, Node, NodeQuery};
    use proximadb_graph_query::service::GraphQueryResult;

    /// A fixed in-memory graph read service — a linear chain over one edge type, enough to
    /// exercise the variable-length `*k..k` traversal the `graph_traverse` provider runs.
    #[derive(Default)]
    struct FixedGraphOps {
        nodes: HashMap<String, Arc<Node>>,
        edges: Vec<Arc<Edge>>,
    }

    impl FixedGraphOps {
        fn chain(ids: &[&str], edge_type: &str) -> Self {
            let mut s = Self::default();
            for id in ids {
                s.nodes.insert(
                    (*id).to_string(),
                    Arc::new(Node {
                        id: (*id).to_string(),
                        labels: vec!["N".to_string()],
                        ..Default::default()
                    }),
                );
            }
            for w in ids.windows(2) {
                s.edges.push(Arc::new(Edge {
                    id: format!("{}->{}", w[0], w[1]),
                    from_node_id: w[0].to_string(),
                    to_node_id: w[1].to_string(),
                    edge_type: edge_type.to_string(),
                    ..Default::default()
                }));
            }
            s
        }
    }

    #[async_trait]
    impl GraphQueryReadService for FixedGraphOps {
        async fn list_graphs(&self) -> GraphQueryResult<Vec<String>> {
            Ok(vec!["g".to_string()])
        }
        async fn get_node(&self, _g: &str, id: &str) -> GraphQueryResult<Option<Arc<Node>>> {
            Ok(self.nodes.get(id).cloned())
        }
        async fn query_nodes(&self, _g: &str, _q: NodeQuery) -> GraphQueryResult<Vec<Arc<Node>>> {
            Ok(self.nodes.values().cloned().collect())
        }
        async fn query_edges(&self, _g: &str, q: EdgeQuery) -> GraphQueryResult<Vec<Arc<Edge>>> {
            Ok(self
                .edges
                .iter()
                .filter(|e| {
                    q.from_node_id
                        .as_deref()
                        .is_none_or(|f| e.from_node_id == f)
                        && (q.edge_types.is_empty()
                            || q.edge_types.iter().any(|t| t == &e.edge_type))
                })
                .cloned()
                .collect())
        }
    }

    /// The moat proof for the third modality: a graph reachability set JOINs a relational table
    /// in ONE DataFusion SQL plan — the shape a pgwire client writes for `relational ⋈ graph`.
    #[tokio::test]
    async fn graph_traverse_udtf_joins_relational_in_sql() {
        // n0 -> n1 -> n2 -> n3 over LINK.
        let graph: Arc<dyn GraphQueryReadService> =
            Arc::new(FixedGraphOps::chain(&["n0", "n1", "n2", "n3"], "LINK"));
        let ctx = SessionContext::new();
        ctx.register_udtf(
            "graph_traverse",
            Arc::new(GraphTraverseTableFunction::new(graph)),
        );

        // `*1..3` from n0 reaches n1 (d1), n2 (d2), n3 (d3).
        let rows: usize = ctx
            .sql("SELECT node_id, depth FROM graph_traverse('g','n0','LINK',3) ORDER BY depth")
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
            .iter()
            .map(|b| b.num_rows())
            .sum();
        assert_eq!(rows, 3);

        // Join reachability with a relational dimension.
        let dim_schema = Arc::new(Schema::new(vec![
            Field::new("node_id", DataType::Utf8, false),
            Field::new("tier", DataType::Utf8, false),
        ]));
        let dim = RecordBatch::try_new(
            dim_schema.clone(),
            vec![
                Arc::new(StringArray::from(vec!["n1", "n3", "z"])),
                Arc::new(StringArray::from(vec!["hot", "hot", "cold"])),
            ],
        )
        .unwrap();
        ctx.register_table(
            "acct",
            Arc::new(MemTable::try_new(dim_schema, vec![vec![dim]]).unwrap()),
        )
        .unwrap();
        let joined: usize = ctx
            .sql(
                "SELECT a.tier, g.depth FROM acct a \
                 JOIN graph_traverse('g','n0','LINK',4) g ON a.node_id = g.node_id",
            )
            .await
            .unwrap()
            .collect()
            .await
            .unwrap()
            .iter()
            .map(|b| b.num_rows())
            .sum();
        assert_eq!(joined, 2); // n1 + n3 reachable & in acct; z has no reachable node
    }

    #[tokio::test]
    async fn graph_traverse_udtf_rejects_invalid_inputs() {
        let graph: Arc<dyn GraphQueryReadService> = Arc::new(FixedGraphOps::default());
        let ctx = SessionContext::new();
        ctx.register_udtf(
            "graph_traverse",
            Arc::new(GraphTraverseTableFunction::new(graph)),
        );

        for (sql, expected) in [
            (
                "SELECT * FROM graph_traverse('','n0','LINK',3)",
                "graph_id must not be empty",
            ),
            (
                "SELECT * FROM graph_traverse('g','','LINK',3)",
                "start_id must not be empty",
            ),
            (
                "SELECT * FROM graph_traverse('g','n0','bad edge',3)",
                "edge_type must be a bare identifier",
            ),
            (
                "SELECT * FROM graph_traverse('g','n0','LINK',0)",
                "max_depth must be greater than zero",
            ),
        ] {
            let error = ctx.sql(sql).await.expect_err("invalid graph_traverse");
            assert!(
                error.to_string().contains(expected),
                "expected {expected:?} in {error}"
            );
        }
    }
}
