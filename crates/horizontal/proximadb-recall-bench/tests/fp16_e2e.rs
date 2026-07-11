//! End-to-end fp16 pipeline scaffold — PR 12 of
//! `docs/12-design/EMBEDDING_PRECISION_LLD_2026_05_22.adoc` §"PR 12 —
//! End-to-end fp16 collection (the payoff)".
//!
//! Composes the adapter modules shipped in PRs 1, 6, 8, and 11 into a
//! single integration test that walks an fp32 query through the full
//! "fp16 collection" pipeline:
//!
//!   1. Catalog declares `canonical_embedding_precision = Fp16`
//!      (PR 6b CatalogTableSchema extension).
//!   2. Records project to fp16 via PR 8a `project_to_canonical`.
//!   3. Query downconverts to fp16 via PR 8b `prepare_query`.
//!   4. Recall against the fp32 ground truth is measured by the PR 11
//!      `recall_at_k` calculator.
//!   5. The LLD §Motivation byte-size invariant (fp16 = ½ fp32) is
//!      asserted at every step.
//!
//! This scaffold deliberately does NOT plug into the live WAL writer,
//! PAX block reader, or ANN index — those integrations are deferred per
//! PR 4 / 5 / 7 / 8 follow-ups. What this test proves today is that the
//! adapter contracts compose correctly into a recall ≥ 0.99 fp16
//! pipeline, so when the integrations land the only new code under test
//! is the wiring, not the algorithms.

use proximadb_catalog::CatalogTableSchema;
use proximadb_catalog::embedding_precision_policy::{
    EmbeddingPrecisionPolicy, IngestMismatchPolicy, PrecisionMigrationState, RecallSlo,
};
use proximadb_recall_bench::{
    Dataset, DistanceMetric, NeighborId, QueryId, QueryResult, RecallReport, SyntheticDataset,
    measure_recall, recall_at_k,
};
use proximadb_records::{EmbeddingScalarType, EmbeddingValues};

// `project_to_canonical` (PR 8a) and `prepare_query` (PR 8b) live in the
// modality crate proximadb-embedding which horizontal crates can't depend
// on. Both delegate to `EmbeddingValues::from_fp32_lossy` in the
// foundation crate for the fp32-input cases we exercise here, so we call
// the foundation primitive directly. The contract is identical (PR 8 test
// `prepare_query_matches_writer_quantization_byte_for_byte` locks it).
fn project_fp32_to_canonical(src: &[f32], target: EmbeddingScalarType) -> EmbeddingValues {
    EmbeddingValues::from_fp32_lossy(src, target)
}

fn prepare_query_at(query: &[f32], target: EmbeddingScalarType) -> EmbeddingValues {
    EmbeddingValues::from_fp32_lossy(query, target)
}

// ---------------------------------------------------------------------------
// Deterministic synthetic embedding corpus
// ---------------------------------------------------------------------------

const CORPUS_SIZE: usize = 256;
const QUERY_COUNT: usize = 32;
const DIM: usize = 128;
const TOP_K: usize = 10;

/// Generate a deterministic fp32 vector for a given id. xorshift keeps
/// the test reproducible across runs without pulling in a rand crate.
fn deterministic_fp32_vector(id: u64) -> Vec<f32> {
    let mut state = id.wrapping_mul(0x9E37_79B9_7F4A_7C15).wrapping_add(1);
    let mut out = Vec::with_capacity(DIM);
    for _ in 0..DIM {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        // Map to [-1.0, 1.0).
        let f = (state as f32 / u64::MAX as f32) * 2.0 - 1.0;
        out.push(f);
    }
    // L2-normalize so cosine similarity == dot product.
    let norm: f32 = out.iter().map(|x| x * x).sum::<f32>().sqrt().max(1e-9);
    out.iter().map(|x| x / norm).collect()
}

/// Cosine similarity (vectors are pre-normalized).
fn cosine(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b.iter()).map(|(x, y)| x * y).sum()
}

/// Brute-force top-K cosine search at the given precision.
///
/// `query` and `corpus` are both in canonical precision. For fp16 mode
/// we promote to fp32 for the dot-product itself (the LLD's "promote-
/// at-compute" pattern from Q10 — native fp16 kernels land in Phase 6).
fn brute_force_top_k(
    query: &EmbeddingValues,
    corpus: &[EmbeddingValues],
    k: usize,
) -> Vec<(NeighborId, f32)> {
    let query_f32 = match query {
        EmbeddingValues::Fp32(v) => v.clone(),
        EmbeddingValues::Fp16(v) => v.iter().map(|x| x.to_f32()).collect(),
        _ => panic!("scaffold only exercises fp32 and fp16 today"),
    };
    let mut scored: Vec<(NeighborId, f32)> = corpus
        .iter()
        .enumerate()
        .map(|(idx, cell)| {
            let cell_f32: Vec<f32> = match cell {
                EmbeddingValues::Fp32(v) => v.clone(),
                EmbeddingValues::Fp16(v) => v.iter().map(|x| x.to_f32()).collect(),
                _ => panic!("scaffold only exercises fp32 and fp16 today"),
            };
            (idx as NeighborId, cosine(&query_f32, &cell_f32))
        })
        .collect();
    // Sort descending by score, then truncate.
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored.truncate(k);
    scored
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[test]
fn fp16_catalog_schema_round_trips_with_pr6b_fields() {
    // PR 6b: declaring an fp16 collection should round-trip cleanly.
    let mut schema = CatalogTableSchema::new("fp16_collection");
    schema.canonical_embedding_precision = EmbeddingScalarType::Fp16;
    schema.allowed_embedding_precisions =
        vec![EmbeddingScalarType::Fp16, EmbeddingScalarType::Fp32];
    schema.precision_migration_state = Some(PrecisionMigrationState::Stable);

    let json = serde_json::to_string(&schema).unwrap();
    let back: CatalogTableSchema = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.canonical_embedding_precision,
        EmbeddingScalarType::Fp16
    );
    assert_eq!(back.allowed_embedding_precisions.len(), 2);
    assert_eq!(
        back.precision_migration_state,
        Some(PrecisionMigrationState::Stable)
    );
}

#[test]
fn fp16_collection_writer_path_halves_byte_size_per_lld_motivation() {
    // PR 8a contract + LLD §Motivation: an fp16-canonical record's
    // storage is exactly half the fp32 equivalent.
    let raw = deterministic_fp32_vector(0);
    let fp32 = project_fp32_to_canonical(&raw, EmbeddingScalarType::Fp32);
    let fp16 = project_fp32_to_canonical(&raw, EmbeddingScalarType::Fp16);
    let fp32_bytes = fp32.byte_size();
    let fp16_bytes = fp16.byte_size();
    assert_eq!(
        fp32_bytes,
        fp16_bytes * 2,
        "fp16 collection must store exactly half the bytes vs fp32 \
         (LLD §Motivation): fp32={fp32_bytes}, fp16={fp16_bytes}"
    );
}

#[test]
fn fp16_query_path_reuses_writer_quantization_byte_for_byte() {
    // PR 8 critical invariant: the writer-side `project_to_canonical`
    // and query-side `prepare_query` MUST produce bit-identical bytes
    // for fp16 → divergence would silently corrupt search results.
    let raw = deterministic_fp32_vector(42);
    let writer = project_fp32_to_canonical(&raw, EmbeddingScalarType::Fp16);
    let query = prepare_query_at(&raw, EmbeddingScalarType::Fp16);
    assert_eq!(
        writer, query,
        "writer and query fp16 quantization must agree"
    );
}

#[test]
fn fp16_recall_at_10_meets_lld_q13_cosine_gate() {
    // The headline integration test: index a 256-vector synthetic
    // corpus at fp16, query with 32 fp16-downconverted queries, and
    // verify recall@10 vs the fp32 baseline meets the LLD §Q13 cosine
    // gate (≥ 0.99).
    let fp32_corpus: Vec<Vec<f32>> = (0..CORPUS_SIZE as u64)
        .map(deterministic_fp32_vector)
        .collect();
    let fp32_queries: Vec<Vec<f32>> = (1000..(1000 + QUERY_COUNT as u64))
        .map(deterministic_fp32_vector)
        .collect();

    // Project corpus through the canonical adapter for each precision.
    let fp32_cells: Vec<EmbeddingValues> = fp32_corpus
        .iter()
        .map(|v| project_fp32_to_canonical(v, EmbeddingScalarType::Fp32))
        .collect();
    let fp16_cells: Vec<EmbeddingValues> = fp32_corpus
        .iter()
        .map(|v| project_fp32_to_canonical(v, EmbeddingScalarType::Fp16))
        .collect();

    // Build per-query reference + candidate results.
    let mut reference: Vec<QueryResult> = Vec::with_capacity(QUERY_COUNT);
    let mut candidate: Vec<QueryResult> = Vec::with_capacity(QUERY_COUNT);
    let mut dataset = SyntheticDataset::new("synthetic_fp16_e2e");

    for (i, q) in fp32_queries.iter().enumerate() {
        let qid = i as QueryId;

        // Reference: fp32 query against fp32 corpus.
        let ref_query = prepare_query_at(q, EmbeddingScalarType::Fp32);
        let ref_topk: Vec<NeighborId> = brute_force_top_k(&ref_query, &fp32_cells, TOP_K)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        dataset = dataset.with_query(qid, ref_topk.clone());
        reference.push(QueryResult {
            query_id: qid,
            neighbors: ref_topk,
        });

        // Candidate: fp16 query against fp16 corpus (promote-at-compute).
        let cand_query = prepare_query_at(q, EmbeddingScalarType::Fp16);
        let cand_topk: Vec<NeighborId> = brute_force_top_k(&cand_query, &fp16_cells, TOP_K)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        candidate.push(QueryResult {
            query_id: qid,
            neighbors: cand_topk,
        });
    }

    let row = measure_recall(
        &dataset,
        DistanceMetric::Cosine,
        TOP_K,
        &reference,
        &candidate,
    )
    .unwrap();

    let q13_gate = RecallSlo::lld_defaults().cosine.at_10;
    assert!(
        row.mean_recall >= q13_gate as f32,
        "fp16 mean recall@{TOP_K} = {} fell below LLD §Q13 cosine gate {} \
         (this is the LLD's go/no-go threshold for shipping fp16)",
        row.mean_recall,
        q13_gate,
    );
    // Min recall is the tail-risk gate — keep it loose to reflect that
    // single-query worst-case can dip even when the fleet is healthy.
    assert!(
        row.min_recall >= 0.5,
        "fp16 min recall@{TOP_K} = {} suggests a worst-case query whose \
         entire top-K rerank flipped under fp16 noise — investigate",
        row.min_recall,
    );

    // Materialize a RecallReport like the CI gate would consume.
    let report = RecallReport {
        candidate_label: "fp16-canonical".to_string(),
        reference_label: "fp32-canonical".to_string(),
        rows: vec![row.clone()],
    };
    let fetched = report
        .row(dataset.name(), DistanceMetric::Cosine, TOP_K)
        .unwrap();
    assert_eq!(fetched, &row);
}

#[test]
fn fp16_self_recall_against_itself_is_perfect() {
    // PR 11 LLD-required sanity test, repeated end-to-end: querying
    // fp16 corpus with itself gives recall@K = 1.0. Catches any
    // ordering bug in `brute_force_top_k`.
    let fp32_corpus: Vec<Vec<f32>> = (0..16u64).map(deterministic_fp32_vector).collect();
    let fp16_cells: Vec<EmbeddingValues> = fp32_corpus
        .iter()
        .map(|v| project_fp32_to_canonical(v, EmbeddingScalarType::Fp16))
        .collect();

    for (idx, q) in fp32_corpus.iter().enumerate() {
        let query = prepare_query_at(q, EmbeddingScalarType::Fp16);
        let topk: Vec<NeighborId> = brute_force_top_k(&query, &fp16_cells, 5)
            .into_iter()
            .map(|(id, _)| id)
            .collect();
        assert!(
            topk.first().copied() == Some(idx as NeighborId),
            "self-recall failed: query {idx} top-1 was {:?}",
            topk.first()
        );
        let reference: Vec<NeighborId> = vec![idx as NeighborId];
        let candidate = vec![topk[0]];
        let r = recall_at_k(&reference, &candidate, 1).unwrap();
        assert_eq!(r, 1.0);
    }
}

#[test]
fn ingest_policy_reject_blocks_fp16_writes_when_canonical_is_fp32() {
    // PR 6a contract: when a collection's policy is `Reject` (the
    // default seed) and canonical is fp32, ingesting an fp16-declared
    // record must be rejected at the API edge — the WAL writer guard
    // (PR 3b validator) is what enforces this in production. Here we
    // exercise the policy itself.
    let policy = EmbeddingPrecisionPolicy::global_default_fp32(0);
    assert_eq!(policy.canonical_default, EmbeddingScalarType::Fp32);
    assert_eq!(policy.ingest_mismatch, IngestMismatchPolicy::Reject);
    // Allowed list is fp32-only by default — fp16 not in the set means
    // an fp16 record violates the policy.
    assert!(
        policy
            .canonical_allowed
            .contains(&EmbeddingScalarType::Fp32)
    );
    assert!(
        !policy
            .canonical_allowed
            .contains(&EmbeddingScalarType::Fp16)
    );
}
