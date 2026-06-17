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
//! ## Scope (this slice)
//! The conversion bridge + a proof that the join executes in one DataFusion plan. The
//! next slices: (a) a `VectorOpsPort`-backed `TableProvider` whose `scan` runs the
//! live search; (b) a frontend `VECTOR_SEARCH(...)` source + a
//! `proximadb-relational-algebra` source node that lowers (via the P4
//! `logical_lowering`) into the shared logical plane so the join is reachable from
//! pgwire SQL. Both reuse [`vector_matches_to_batch`] below.

use std::any::Any;
use std::sync::Arc;

use arrow_array::{Float32Array, RecordBatch, StringArray};
use arrow_schema::{ArrowError, DataType, Field, Schema, SchemaRef};
use async_trait::async_trait;
use datafusion::catalog::{Session, TableFunctionImpl};
use datafusion::datasource::{MemTable, TableProvider, TableType};
use datafusion::error::{DataFusionError, Result as DFResult};
use datafusion::logical_expr::Expr;
use datafusion::physical_plan::ExecutionPlan;
use datafusion::scalar::ScalarValue;

use crate::proto::proximadb_v1::{SearchQuery, SearchVectorRecord, VectorSearchRequest};

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
    fn as_any(&self) -> &dyn Any {
        self
    }

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
}

impl VectorSearchTableFunction {
    /// Capture the live vector service the function will search.
    pub fn new(vector_ops: Arc<dyn proximadb_runtime::VectorOpsPort>) -> Self {
        Self { vector_ops }
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
            None,
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
}
