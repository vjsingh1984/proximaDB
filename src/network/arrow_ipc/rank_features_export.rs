//! Arrow Flight `rank_features_export` action (R-7c.4b).
//!
//! Streams the multi-phase ranking pipeline's per-doc match_features as
//! an Arrow RecordBatch over Flight, suitable for offline learning-to-
//! rank training data. The wire shape complements the REST
//! `match_features` HashMap (R-7c.5): same data, but in columnar Arrow
//! form so a downstream pipeline can hand-off directly into Parquet /
//! Iceberg without server-side JSON parsing.
//!
//! Spec: roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md §4.9.3.
//!
//! Wire contract — request body (JSON):
//! ```json
//! {
//!   "collection": "docs",
//!   "query_vector": [...],
//!   "query_text": "...",
//!   "k": 50,
//!   "rank_profile": "semantic_plus_ce",
//!   "rank_overrides": { ... }
//! }
//! ```
//! Same shape REST `/v1/rank/search` accepts, so SDKs can lift their
//! existing serialisation.
//!
//! Wire contract — response (one Arrow IPC stream):
//! ```text
//! Schema:
//!   id              Utf8                  // doc id (round-trips through original_ids)
//!   rank            UInt32                // 0-based position in the response
//!   score           Float32               // post-pipeline primary score
//!   phase           UInt8                 // PhaseId of the score (0/1/2)
//!   <feature_N>     Float64 (nullable)    // one column per profile.match_features entry
//! ```
//! Columns 1..K are stable across the response — column order matches
//! the profile's declared `match_features` order. Missing features (the
//! hit didn't carry that feature) encode as null in their column.
//!
//! No buffering / streaming policy: v1 returns one Arrow IPC stream
//! with a single batch. The rank pipeline already truncates to `k`, so
//! the result set fits in memory. A streaming follow-up
//! (R-7c.4b.1) can split into chunks once we have a producer that
//! emits multiple `PhaseOutcome`s.

use std::collections::BTreeMap;
use std::sync::Arc;

use arrow_array::{
    ArrayRef, Float32Array, Float64Array, RecordBatch, StringArray, UInt32Array, UInt8Array,
};
use arrow_ipc::writer::StreamWriter;
use arrow_schema::{DataType, Field, Schema};

use crate::network::rest::v1::rank::{
    handle_rank_search, RankSearchRequest, RankSearchResponse, RankServices,
};
use proximadb_rank_core::RankError;
use proximadb_rank_profile::CompiledRankProfile;

/// Decode the Flight action body, drive the rank pipeline, and return
/// the response data as Arrow IPC stream bytes.
///
/// `services` carries the per-process `RankServices` singleton — same
/// instance the REST + gRPC handlers use, so behaviour stays in lock-
/// step across all three protocols.
///
/// Errors:
/// - body parse failure → returns the descriptive error message
/// - profile not found / invalid → propagated from handle_rank_search
/// - request without a profile → still works (returns hits with no
///   match_features columns; the schema is just `id/rank/score/phase`)
/// - Arrow IPC encode failure → wraps as `RankError::ModelInference`
pub async fn export_rank_features_to_arrow_ipc(
    services: &Arc<RankServices>,
    body: &[u8],
) -> Result<Vec<u8>, RankError> {
    let request: RankSearchRequest = serde_json::from_slice(body).map_err(|e| {
        RankError::InvalidProfile(format!(
            "rank_features_export: request body must be JSON RankSearchRequest: {e}"
        ))
    })?;

    // Resolve match_features column order from the active profile BEFORE
    // the pipeline runs — we need the canonical column order to keep the
    // Arrow schema stable across requests for the same profile (a row
    // missing a feature emits null in that column rather than reshaping
    // the schema). Profiles without a rank_profile name use an empty
    // column list (id/rank/score/phase only).
    let column_names: Vec<String> = request
        .rank_profile
        .as_deref()
        .and_then(|name| services.profile_registry.get(name))
        .map(profile_match_feature_names)
        .unwrap_or_default();

    let second_phase_scorer = request
        .rank_profile
        .as_deref()
        .and_then(|name| services.second_phase_scorer(name));

    let response = handle_rank_search(
        request,
        services.profile_registry.as_ref(),
        services.candidate_provider.as_ref(),
        services.blueprint_factory.clone(),
        second_phase_scorer,
    )
    .await?;

    let batch = build_record_batch(response, &column_names).map_err(|e| {
        RankError::ModelInference {
            model_id: "rank_features_export:arrow_build".into(),
            reason: e,
        }
    })?;
    encode_ipc_stream(&batch).map_err(|e| RankError::ModelInference {
        model_id: "rank_features_export:ipc_encode".into(),
        reason: e,
    })
}

/// Canonical match-feature column order: the profile spec's declared
/// `match_features` list, in declaration order. Acts as the Arrow
/// schema's stable column ordering. Public so server-side tests and
/// future schema-introspection RPCs can compute the same ordering.
pub fn profile_match_feature_names(profile: Arc<CompiledRankProfile>) -> Vec<String> {
    profile
        .spec
        .match_features
        .iter()
        .cloned()
        .collect::<Vec<_>>()
}

fn build_record_batch(
    response: RankSearchResponse,
    column_names: &[String],
) -> Result<RecordBatch, String> {
    let n = response.hits.len();

    let id_col: ArrayRef = Arc::new(StringArray::from(
        response.hits.iter().map(|h| h.id.as_str()).collect::<Vec<_>>(),
    ));
    let rank_col: ArrayRef = Arc::new(UInt32Array::from(
        (0u32..n as u32).collect::<Vec<u32>>(),
    ));
    let score_col: ArrayRef = Arc::new(Float32Array::from(
        response.hits.iter().map(|h| h.score).collect::<Vec<_>>(),
    ));
    let phase_col: ArrayRef = Arc::new(UInt8Array::from(
        response
            .hits
            .iter()
            .map(|h| h.score_vector.as_ref().map(|sv| sv.phase).unwrap_or(0u8))
            .collect::<Vec<_>>(),
    ));

    // Per-feature columns: collect values in row order; missing values
    // emit null so the column shape stays stable even when a hit didn't
    // carry the feature. Use BTreeMap-free indexing — column_names
    // already pins the order.
    let mut feature_columns: Vec<ArrayRef> = Vec::with_capacity(column_names.len());
    for name in column_names {
        let values: Vec<Option<f64>> = response
            .hits
            .iter()
            .map(|h| h.match_features.get(name).copied())
            .collect();
        feature_columns.push(Arc::new(Float64Array::from(values)) as ArrayRef);
    }

    // Build the schema in matching column order.
    let mut fields: Vec<Field> = vec![
        Field::new("id", DataType::Utf8, false),
        Field::new("rank", DataType::UInt32, false),
        Field::new("score", DataType::Float32, false),
        Field::new("phase", DataType::UInt8, false),
    ];
    for name in column_names {
        // Per-feature columns are nullable — a hit that didn't carry a
        // feature value writes NULL rather than reshaping the schema.
        fields.push(Field::new(name, DataType::Float64, true));
    }
    let schema = Arc::new(Schema::new(fields));

    let mut columns: Vec<ArrayRef> = vec![id_col, rank_col, score_col, phase_col];
    columns.extend(feature_columns);
    RecordBatch::try_new(schema, columns).map_err(|e| format!("RecordBatch::try_new: {e}"))
}

fn encode_ipc_stream(batch: &RecordBatch) -> Result<Vec<u8>, String> {
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut writer = StreamWriter::try_new(&mut buf, batch.schema().as_ref())
            .map_err(|e| format!("StreamWriter::try_new: {e}"))?;
        writer
            .write(batch)
            .map_err(|e| format!("StreamWriter::write: {e}"))?;
        writer
            .finish()
            .map_err(|e| format!("StreamWriter::finish: {e}"))?;
    }
    Ok(buf)
}

// `BTreeMap` is referenced in doc-context only; keep the import quiet.
#[allow(dead_code)]
fn _unused_btreemap_marker(_: BTreeMap<String, ()>) {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::network::rest::v1::rank::{
        CandidateBatch, CandidateProvider, ScoredHitDto, ScoreVectorDto,
    };
    use arrow_array::cast::AsArray;
    use arrow_ipc::reader::StreamReader;
    use proximadb_kernel::ScoreComponent;
    use proximadb_rank_core::{
        Blueprint, BlueprintFactory, DocHandle, FeatureExecutor, FeatureLookup, OutputSpec,
        PhaseConfig, QueryContext, RankResult, ScoreCtx,
    };
    use proximadb_rank_features::register_builtins;
    use proximadb_rank_profile::{
        CompiledRankProfile, PhaseSpec, ProfileRegistry, RankProfileSpec,
    };
    use std::collections::HashMap;
    use std::sync::Arc;

    // ---------------- Pure-function tests (no rank pipeline) ----------------

    fn dto(id: &str, score: f32, phase: u8, features: &[(&str, f64)]) -> ScoredHitDto {
        let mut mf = HashMap::new();
        for (k, v) in features {
            mf.insert((*k).into(), *v);
        }
        ScoredHitDto {
            id: id.into(),
            score,
            score_vector: Some(ScoreVectorDto {
                primary: score,
                phase,
                components: Vec::<ScoreComponent>::new(),
            }),
            match_features: mf,
            summary_features: HashMap::new(),
        }
    }

    fn resp(hits: Vec<ScoredHitDto>) -> RankSearchResponse {
        RankSearchResponse {
            hits,
            phase_truncated: false,
            rank_profile: Some("test".into()),
            rank_profile_version: Some(1),
        }
    }

    #[test]
    fn build_batch_with_no_features_emits_4_column_schema() {
        let r = resp(vec![
            dto("a", 0.9, 0, &[]),
            dto("b", 0.7, 0, &[]),
        ]);
        let batch = build_record_batch(r, &[]).unwrap();
        let s = batch.schema();
        assert_eq!(s.fields().len(), 4);
        assert_eq!(s.field(0).name(), "id");
        assert_eq!(s.field(1).name(), "rank");
        assert_eq!(s.field(2).name(), "score");
        assert_eq!(s.field(3).name(), "phase");
        assert_eq!(batch.num_rows(), 2);
    }

    #[test]
    fn build_batch_preserves_column_order_from_profile_declaration() {
        let r = resp(vec![dto(
            "a",
            0.9,
            0,
            &[("bm25(title)", 12.0), ("closeness(embedding)", 0.91)],
        )]);
        let batch =
            build_record_batch(r, &["bm25(title)".into(), "closeness(embedding)".into()])
                .unwrap();
        let s = batch.schema();
        assert_eq!(s.fields().len(), 6);
        assert_eq!(s.field(4).name(), "bm25(title)");
        assert_eq!(s.field(5).name(), "closeness(embedding)");
    }

    #[test]
    fn build_batch_emits_null_for_missing_feature_on_a_row() {
        // Row "a" has feature X, row "b" doesn't. Column X for row "b"
        // must encode as null rather than reshape the schema.
        let r = resp(vec![
            dto("a", 0.9, 0, &[("X", 1.5)]),
            dto("b", 0.7, 0, &[]),
        ]);
        let batch = build_record_batch(r, &["X".into()]).unwrap();
        let col = batch.column(4);
        let arr = col.as_primitive::<arrow_array::types::Float64Type>();
        assert_eq!(arr.value(0), 1.5);
        assert!(arr.is_null(1), "row 'b' missing feature X must encode null");
    }

    #[test]
    fn build_batch_assigns_rank_as_zero_based_row_index() {
        let r = resp(vec![
            dto("a", 0.9, 0, &[]),
            dto("b", 0.7, 0, &[]),
            dto("c", 0.5, 0, &[]),
        ]);
        let batch = build_record_batch(r, &[]).unwrap();
        let rank = batch
            .column(1)
            .as_primitive::<arrow_array::types::UInt32Type>();
        assert_eq!(rank.value(0), 0);
        assert_eq!(rank.value(1), 1);
        assert_eq!(rank.value(2), 2);
    }

    #[test]
    fn ipc_round_trip_preserves_rows_columns_and_values() {
        // Encode → decode the same data and assert structural equality
        // of the columns we care about (id + score + per-feature value).
        let r = resp(vec![
            dto("doc:1", 0.8, 1, &[("F", 3.5)]),
            dto("doc:2", 0.6, 1, &[("F", 2.5)]),
        ]);
        let batch = build_record_batch(r, &["F".into()]).unwrap();
        let bytes = encode_ipc_stream(&batch).unwrap();

        let mut reader = StreamReader::try_new(bytes.as_slice(), None).unwrap();
        let back = reader.next().unwrap().unwrap();
        assert_eq!(back.num_rows(), 2);
        assert_eq!(back.num_columns(), 5); // id/rank/score/phase + F
        let ids = back.column(0).as_string::<i32>();
        assert_eq!(ids.value(0), "doc:1");
        assert_eq!(ids.value(1), "doc:2");
        let f = back
            .column(4)
            .as_primitive::<arrow_array::types::Float64Type>();
        assert_eq!(f.value(0), 3.5);
        assert_eq!(f.value(1), 2.5);
    }

    // ---------------- End-to-end: full pipeline → Arrow IPC bytes ----------------

    struct DocIdExec;
    impl FeatureExecutor for DocIdExec {
        fn execute(
            &mut self,
            doc: DocHandle,
            _lookup: &mut dyn FeatureLookup,
            _ctx: &mut ScoreCtx<'_>,
        ) -> f32 {
            doc.0 as f32
        }
    }
    struct DocIdBp;
    impl Blueprint for DocIdBp {
        fn name(&self) -> &str {
            "docid"
        }
        fn declared_outputs(&self) -> &[OutputSpec] {
            &[]
        }
        fn build_executor(
            &self,
            _cfg: &PhaseConfig,
            _q: &QueryContext,
        ) -> RankResult<Box<dyn FeatureExecutor>> {
            Ok(Box::new(DocIdExec))
        }
    }
    struct FixedCandidates(Vec<DocHandle>);
    #[async_trait::async_trait]
    impl CandidateProvider for FixedCandidates {
        async fn candidates(&self, _r: &RankSearchRequest) -> RankResult<CandidateBatch> {
            Ok(CandidateBatch::from_docs(self.0.clone()))
        }
    }

    fn factory_with_docid() -> Arc<BlueprintFactory> {
        let f = Arc::new(BlueprintFactory::new());
        register_builtins(&f);
        f.register(Arc::new(DocIdBp));
        f
    }

    fn rank_services_with_profile(
        name: &str,
        match_features: Vec<&str>,
        candidates: Arc<dyn CandidateProvider>,
    ) -> Arc<RankServices> {
        let factory = factory_with_docid();
        let registry = ProfileRegistry::new();
        let mut spec = RankProfileSpec::new(name);
        spec.first_phase = Some(PhaseSpec {
            expression: "docid()".into(),
            heap_size: Some(50),
            rerank_count: None,
            batch_size: None,
        });
        spec.match_features = match_features.into_iter().map(String::from).collect();
        spec.version = 1;
        let compiled = CompiledRankProfile::compile(spec, factory.clone()).unwrap();
        registry.install(compiled);
        Arc::new(RankServices {
            profile_registry: Arc::new(registry),
            blueprint_factory: factory,
            candidate_provider: candidates,
            second_phase_scorers: dashmap::DashMap::new(),
        })
    }

    #[tokio::test]
    async fn export_drives_handler_and_emits_per_doc_feature_rows() {
        let candidates: Arc<dyn CandidateProvider> =
            Arc::new(FixedCandidates(vec![DocHandle(3), DocHandle(8)]));
        let services = rank_services_with_profile("p", vec!["docid()", "1.0"], candidates);

        let body = serde_json::to_vec(&serde_json::json!({
            "collection": "docs",
            "query_vector": [],
            "k": 2,
            "rank_profile": "p",
        }))
        .unwrap();

        let ipc = export_rank_features_to_arrow_ipc(&services, &body).await.unwrap();
        let mut reader = StreamReader::try_new(ipc.as_slice(), None).unwrap();
        let batch = reader.next().unwrap().unwrap();
        assert_eq!(batch.num_rows(), 2);
        assert_eq!(batch.num_columns(), 6); // id/rank/score/phase + docid() + 1.0

        let s = batch.schema();
        assert_eq!(s.field(4).name(), "docid()");
        assert_eq!(s.field(5).name(), "1.0");

        // Top hit is doc 8 (docid scorer).
        let ids = batch.column(0).as_string::<i32>();
        assert_eq!(ids.value(0), "8");
        let docid_col = batch
            .column(4)
            .as_primitive::<arrow_array::types::Float64Type>();
        assert_eq!(docid_col.value(0), 8.0);
        assert_eq!(docid_col.value(1), 3.0);
        let one_col = batch
            .column(5)
            .as_primitive::<arrow_array::types::Float64Type>();
        assert_eq!(one_col.value(0), 1.0);
        assert_eq!(one_col.value(1), 1.0);
    }

    #[tokio::test]
    async fn export_without_profile_falls_back_to_id_score_only_schema() {
        // Body has no rank_profile → retrieval-only path; the schema
        // drops to id/rank/score/phase (no per-feature columns).
        let candidates: Arc<dyn CandidateProvider> =
            Arc::new(FixedCandidates(vec![DocHandle(1), DocHandle(2)]));
        let services = Arc::new(RankServices::new(candidates));

        let body = serde_json::to_vec(&serde_json::json!({
            "collection": "docs",
            "query_vector": [],
            "k": 2,
        }))
        .unwrap();
        let ipc = export_rank_features_to_arrow_ipc(&services, &body).await.unwrap();
        let mut reader = StreamReader::try_new(ipc.as_slice(), None).unwrap();
        let batch = reader.next().unwrap().unwrap();
        assert_eq!(batch.num_columns(), 4); // id/rank/score/phase only
        assert_eq!(batch.num_rows(), 2);
    }

    #[tokio::test]
    async fn export_with_bad_body_returns_invalid_profile_error() {
        let candidates: Arc<dyn CandidateProvider> =
            Arc::new(FixedCandidates(vec![DocHandle(1)]));
        let services = Arc::new(RankServices::new(candidates));
        let err = export_rank_features_to_arrow_ipc(&services, b"not json").await.err().unwrap();
        match err {
            RankError::InvalidProfile(msg) => {
                assert!(msg.contains("rank_features_export"));
            }
            other => panic!("expected InvalidProfile, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn export_with_unknown_profile_propagates_profile_not_found() {
        let candidates: Arc<dyn CandidateProvider> =
            Arc::new(FixedCandidates(vec![DocHandle(1)]));
        let services = Arc::new(RankServices::new(candidates));
        let body = serde_json::to_vec(&serde_json::json!({
            "collection": "docs",
            "query_vector": [],
            "k": 1,
            "rank_profile": "ghost",
        }))
        .unwrap();
        let err = export_rank_features_to_arrow_ipc(&services, &body).await.err().unwrap();
        match err {
            RankError::ProfileNotFound(name) => assert_eq!(name, "ghost"),
            other => panic!("expected ProfileNotFound, got {other:?}"),
        }
    }

    #[test]
    fn profile_match_feature_names_returns_declaration_order() {
        let f = factory_with_docid();
        let mut spec = RankProfileSpec::new("o");
        spec.first_phase = Some(PhaseSpec {
            expression: "docid()".into(),
            heap_size: Some(10),
            rerank_count: None,
            batch_size: None,
        });
        spec.match_features = vec!["alpha".into(), "beta".into(), "gamma".into()];
        let compiled = Arc::new(CompiledRankProfile::compile(spec, f).unwrap());
        let names = profile_match_feature_names(compiled);
        assert_eq!(names, vec!["alpha", "beta", "gamma"]);
    }
}
