//! Serving-path ONNX parity for the pinned cross-encoder rerankers (TD-SELECTOR-1, gate 5).
//!
//! PR #1726's reranker evidence was produced by the *Python* path
//! (`sentence-transformers` CrossEncoder, MPS fp32); the ledger entry records
//! "serving ONNX parity and latency remain unmeasured". This test closes the
//! parity half for every model in the pinned catalog: the serving stack
//! (`BertPairTokenizingDocFeatureExtractor` → `TokenizedBatch` →
//! `OrtTokenizedScorerSession`, i.e. the exact tokenizer-config + session path
//! production uses) must reproduce the reference scores.
//!
//! Two models are pinned, exercising both halves of the session contract:
//!
//! - `cross-encoder/ms-marco-MiniLM-L6-v2` @ `4bebbd56` — BERT family, **3**
//!   input slots (`token_type_ids` bound), pad id 0. Env gate:
//!   `PROXIMADB_TEST_BERT_ONNX_PATH`.
//! - `BAAI/bge-reranker-large` @ `55611d7b` — XLM-R family, **2** input slots
//!   (no `token_type_ids`), pad id 1. Env gate: `PROXIMADB_TEST_BGE_ONNX_PATH`.
//!   The artifact spans graph + external weights (`model.onnx_data` beside the
//!   graph); its content hash is the combined digest of both files.
//!
//! Reference scores live in `tests/fixtures/*_parity_fixture.json` — 64
//! deterministic pairs per model, scored on the Python evidence path, including
//! long docs so the batch-width behavior is exercised.
//!
//! Measured agreement at fixture-generation time (MPS fp32 reference vs
//! onnxruntime CPU fp32): MiniLM max |Δ| = 6.8e-06 / Spearman 1.000000; BGE
//! max |Δ| = 1.7e-05 / Spearman 0.999989. The assertion tolerances below
//! (1e-4 absolute, Spearman ≥ 0.999) sit well above both — loose enough for
//! cross-platform ort variance, tight enough that a real binding/dtype/segment
//! bug (which shifts logits by whole units) cannot pass.
//!
//! ## Running
//!
//! Models are large binaries and live outside the repo (see the catalog fixture
//! for uris). Export them once from the pinned revisions, then per model:
//!
//! ```text
//! PROXIMADB_TEST_BERT_ONNX_PATH=<dir>/model.onnx   # and/or
//! PROXIMADB_TEST_BGE_ONNX_PATH=<dir>/model.onnx \
//!   cargo test -p proximadb-rank-onnx --features real-onnx,bert-tokenizer \
//!   --test onnx_parity -- --nocapture
//! ```
//!
//! `tokenizer.json` is expected beside each model file. Without the env vars
//! the tests skip (registered test-only gates), so default CI stays green with
//! no fixtures on disk.
//!
//! ## Parity-discovery notes
//!
//! - The serving extractor REJECTS pairs that overflow the sequence budget
//!   ("split the document before reranking" — the zero-truncation contract),
//!   whereas the #1726 Python gate path asked the tokenizer to truncate.
//!   Serving is the stricter, safer direction; parity is asserted on
//!   within-budget pairs, and window-splitting of oversized docs is a caller
//!   responsibility on the serving path.
//! - Rows keep their true tokenized length; the extractor pads a batch to its
//!   longest row (TD-SELECTOR-1 gate 5: padding to the full budget was ~11×
//!   wasted compute). Callers merging rows from separate single-row extracts
//!   must pad to their own batch max — the `pad_to_width` helper below is that
//!   pattern.

#![cfg(all(feature = "real-onnx", feature = "bert-tokenizer"))]

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use serde::Deserialize;

use proximadb_rank_core::{DocHandle, QueryContext};
use proximadb_rank_onnx::bert_pair_tokenizing_extractor::{
    BertPairTokenizingDocFeatureExtractor, HashMapDocTextSource,
};
use proximadb_rank_onnx::descriptor::{
    DType, ModelDescriptor, ModelFramework, ModelKey, TensorIoSpec,
};
use proximadb_rank_onnx::ort_tokenized_scorer_session::OrtTokenizedScorerSession;
use proximadb_rank_onnx::tokenized_doc_feature_extractor::TokenizedDocFeatureExtractor;
use proximadb_rank_onnx::tokenized_scorer_session::{TokenizedBatch, TokenizedScorerSession};

/// Absolute score tolerance. Empirical max |Δ| at generation time: 6.8e-06
/// (MiniLM), 1.7e-05 (BGE).
const ABS_TOLERANCE: f64 = 1e-4;
/// Rank-agreement floor. Empirical Spearman at generation time: 1.000000
/// (MiniLM), 0.999989 (BGE — one adjacent swap from cross-device fp noise).
const SPEARMAN_MIN: f64 = 0.999;

#[derive(Deserialize)]
struct Fixture {
    model: String,
    revision: String,
    onnx_sha256: String,
    /// Slot names the session binds, in order — drives the descriptor and
    /// whether `token_type_ids` is emitted.
    #[serde(default)]
    input_slots: Vec<String>,
    max_seq_len: usize,
    pairs: Vec<Pair>,
    reference_scores: Vec<f32>,
}

#[derive(Deserialize)]
struct Pair {
    query: String,
    doc: String,
}

/// Everything one parity run needs.
struct ParityCase {
    /// Env-gated model path; the case is only built when it exists.
    model_path: PathBuf,
    tokenizer_path: PathBuf,
    fixture: Fixture,
}

impl ParityCase {
    /// Resolve (model, tokenizer, fixture) from the env gate, or skip (None).
    fn load(env_var: &str, fixture_src: &str) -> Option<Self> {
        let Ok(model) = std::env::var(env_var) else {
            eprintln!("skipping parity for this model: {env_var} not set");
            return None;
        };
        let model_path = PathBuf::from(model);
        if !model_path.exists() {
            eprintln!("skipping: {model_path:?} does not exist");
            return None;
        }
        let tokenizer_path = model_path.with_file_name("tokenizer.json");
        if !tokenizer_path.exists() {
            eprintln!("skipping: {tokenizer_path:?} does not exist (export it beside the model)");
            return None;
        }
        let mut fixture: Fixture = serde_json::from_str(fixture_src)
            .unwrap_or_else(|e| panic!("checked-in fixture must parse: {e}"));
        // Older fixtures predate the explicit slot list; BERT default is 3-slot.
        if fixture.input_slots.is_empty() {
            fixture.input_slots = vec![
                "input_ids".into(),
                "attention_mask".into(),
                "token_type_ids".into(),
            ];
        }
        assert!(
            (2..=3).contains(&fixture.input_slots.len()),
            "{}: fixture pins 2 or 3 input slots",
            fixture.model
        );
        Some(Self {
            model_path,
            tokenizer_path,
            fixture,
        })
    }

    fn emits_token_type_ids(&self) -> bool {
        self.fixture.input_slots.len() == 3
    }

    fn descriptor(&self) -> ModelDescriptor {
        let max_len = self.fixture.max_seq_len as i64;
        ModelDescriptor {
            key: ModelKey::new(
                self.fixture.model.clone(),
                self.fixture.revision[..7].to_string(),
            ),
            tenant: None,
            uri: format!("file://{}", self.model_path.display()),
            sha256: [0; 32],
            size_bytes: 0,
            framework: ModelFramework::Onnx,
            dtype: DType::Fp32,
            input_spec: self
                .fixture
                .input_slots
                .iter()
                .map(|name| TensorIoSpec {
                    name: name.clone(),
                    shape: vec![None, Some(max_len)],
                    dtype: DType::Fp32,
                })
                .collect(),
            output_spec: vec![TensorIoSpec {
                name: "logits".into(),
                shape: vec![None],
                dtype: DType::Fp32,
            }],
            max_batch_size: 64,
            seq: 0,
            created_at_ms: 0,
        }
    }
}

/// Pad per-row extracts to a common width so separately-extracted rows merge
/// into one rectangular batch. A single-row extract is padded only to its own
/// length, so merging requires this step. Values match what the tokenizer would
/// have padded with: the model's pad id for `input_ids`, 0 (masked) for
/// attention/type rows.
fn pad_to_width(rows: &mut [Vec<i64>], width: usize, pad: i64) {
    for row in rows.iter_mut() {
        row.resize(width, pad);
    }
}

/// Tokenize every fixture pair through the production extractor and score them
/// in one 64-row batch. Each pair has its own query, and the extractor pairs one
/// query against many docs, so each pair becomes a single-row extract; rows are
/// merged manually (padded to the fixture-wide max, ≤ the model's budget).
fn score_fixture(case: &ParityCase) -> Vec<f32> {
    let fixture = &case.fixture;
    let tokenizer = tokenizers::Tokenizer::from_file(&case.tokenizer_path)
        .unwrap_or_else(|e| panic!("tokenizer load {:?}: {e}", case.tokenizer_path));
    let mut texts = HashMapDocTextSource::new();
    for (i, pair) in fixture.pairs.iter().enumerate() {
        texts.insert(DocHandle(i as u32), pair.doc.clone());
    }
    let extractor = BertPairTokenizingDocFeatureExtractor::new(
        Arc::new(tokenizer),
        Arc::new(texts),
        fixture.max_seq_len,
        case.emits_token_type_ids(),
    );

    let mut input_ids: Vec<Vec<i64>> = Vec::with_capacity(fixture.pairs.len());
    let mut attention_mask: Vec<Vec<i64>> = Vec::with_capacity(fixture.pairs.len());
    let mut token_type_ids: Vec<Vec<i64>> = Vec::with_capacity(fixture.pairs.len());

    for (i, pair) in fixture.pairs.iter().enumerate() {
        let qctx = QueryContext {
            query_text: Some(Arc::from(pair.query.as_str())),
            ..QueryContext::default()
        };
        let batch = extractor
            .extract_batch(&[DocHandle(i as u32)], &qctx)
            .unwrap_or_else(|e| panic!("extract pair {i} failed: {e}"));
        assert_eq!(batch.batch_size(), 1, "one doc in, one row out");
        input_ids.push(batch.input_ids[0].clone());
        attention_mask.push(batch.attention_mask[0].clone());
        if let Some(tti) = batch.token_type_ids.as_ref() {
            token_type_ids.push(tti[0].clone());
        } else {
            assert!(
                !case.emits_token_type_ids(),
                "3-slot model requires token_type_ids (pair {i})"
            );
        }
    }

    let width = input_ids.iter().map(Vec::len).max().unwrap_or(0);
    assert!(
        width <= fixture.max_seq_len,
        "merged width {width} exceeds the {} budget",
        fixture.max_seq_len
    );
    pad_to_width(&mut input_ids, width, 0);
    pad_to_width(&mut attention_mask, width, 0);
    pad_to_width(&mut token_type_ids, width, 0);

    let batch = TokenizedBatch {
        input_ids,
        attention_mask,
        token_type_ids: case.emits_token_type_ids().then_some(token_type_ids),
    };
    batch
        .validate_rectangular()
        .unwrap_or_else(|e| panic!("merged batch must be rectangular: {e}"));

    let session = OrtTokenizedScorerSession::load_from_file(case.descriptor(), &case.model_path)
        .unwrap_or_else(|e| panic!("session load {:?}: {e}", case.model_path));
    session
        .score(&batch)
        .unwrap_or_else(|e| panic!("session.score over {} rows failed: {e}", batch.batch_size()))
}

/// Spearman rank correlation over paired scores (ties → average ranks).
fn spearman(a: &[f32], b: &[f32]) -> f64 {
    assert_eq!(a.len(), b.len());
    let rank = |v: &[f32]| -> Vec<f64> {
        let mut order: Vec<usize> = (0..v.len()).collect();
        order.sort_by(|&i, &j| v[i].partial_cmp(&v[j]).unwrap());
        let mut ranks = vec![0.0_f64; v.len()];
        let mut i = 0;
        while i < order.len() {
            let mut j = i;
            while j + 1 < order.len() && v[order[j + 1]] == v[order[i]] {
                j += 1;
            }
            let avg = (i + j) as f64 / 2.0 + 1.0;
            for k in i..=j {
                ranks[order[k]] = avg;
            }
            i = j + 1;
        }
        ranks
    };
    let (ra, rb) = (rank(a), rank(b));
    let n = a.len() as f64;
    let (ma, mb) = (ra.iter().sum::<f64>() / n, rb.iter().sum::<f64>() / n);
    let mut cov = 0.0;
    let mut va = 0.0;
    let mut vb = 0.0;
    for i in 0..a.len() {
        cov += (ra[i] - ma) * (rb[i] - mb);
        va += (ra[i] - ma).powi(2);
        vb += (rb[i] - mb).powi(2);
    }
    if va == 0.0 || vb == 0.0 {
        return 0.0;
    }
    cov / (va * vb).sqrt()
}

/// The shared parity assertion.
fn assert_parity(case: &ParityCase) {
    assert_eq!(
        case.fixture.pairs.len(),
        case.fixture.reference_scores.len()
    );
    let scores = score_fixture(case);
    assert_eq!(scores.len(), case.fixture.reference_scores.len());

    let mut worst = 0.0_f64;
    let mut worst_i = 0;
    for (i, (got, want)) in scores
        .iter()
        .zip(case.fixture.reference_scores.iter())
        .enumerate()
    {
        let delta = (*got as f64 - *want as f64).abs();
        if delta > worst {
            worst = delta;
            worst_i = i;
        }
        assert!(
            delta <= ABS_TOLERANCE,
            "{} pair {i} ('{}…'): serving score {got:.6} vs reference {want:.6}, |Δ| = {delta:.2e} > {ABS_TOLERANCE}",
            case.fixture.model,
            case.fixture.pairs[i].query,
        );
    }
    let rho = spearman(&scores, &case.fixture.reference_scores);
    assert!(
        rho >= SPEARMAN_MIN,
        "{}: rank agreement too low: Spearman {rho:.6} < {SPEARMAN_MIN}",
        case.fixture.model
    );
    println!(
        "{}: parity OK over {} pairs: max |Δ| = {worst:.3e} (pair {worst_i}), Spearman = {rho:.6} @ {} sha256 {}…",
        case.fixture.model,
        scores.len(),
        case.fixture.revision,
        &case.fixture.onnx_sha256[..16]
    );
}

#[test]
fn minilm_serving_onnx_matches_python_evidence_scores() {
    if let Some(case) = ParityCase::load(
        "PROXIMADB_TEST_BERT_ONNX_PATH",
        include_str!("fixtures/onnx_parity_fixture.json"),
    ) {
        assert_parity(&case);
    }
}

#[test]
fn bge_serving_onnx_matches_python_evidence_scores() {
    if let Some(case) = ParityCase::load(
        "PROXIMADB_TEST_BGE_ONNX_PATH",
        include_str!("fixtures/bge_parity_fixture.json"),
    ) {
        assert_parity(&case);
    }
}

/// Latency profile of the serving path per model — one batched extract+score of
/// a single query's rerank window, repeated, reporting mean / p50 / p95. The
/// numbers are machine-specific and NOT evidence-ledger material; the value is
/// the shape (window scaling, model-to-model cost ratio) and a regression signal
/// when run on the same box.
///
/// Run with `--nocapture` to see the tables.
#[test]
fn serving_onnx_latency_profile() {
    if let Some(case) = ParityCase::load(
        "PROXIMADB_TEST_BERT_ONNX_PATH",
        include_str!("fixtures/onnx_parity_fixture.json"),
    ) {
        latency_profile(&case, "minilm-l6-v2");
    }
    if let Some(case) = ParityCase::load(
        "PROXIMADB_TEST_BGE_ONNX_PATH",
        include_str!("fixtures/bge_parity_fixture.json"),
    ) {
        latency_profile(&case, "bge-reranker-large");
    }
}

fn latency_profile(case: &ParityCase, label: &str) {
    let fixture = &case.fixture;
    let tokenizer = tokenizers::Tokenizer::from_file(&case.tokenizer_path)
        .unwrap_or_else(|e| panic!("tokenizer load: {e}"));
    let mut texts = HashMapDocTextSource::new();
    for (i, pair) in fixture.pairs.iter().enumerate() {
        texts.insert(DocHandle(i as u32), pair.doc.clone());
    }
    let extractor = BertPairTokenizingDocFeatureExtractor::new(
        Arc::new(tokenizer),
        Arc::new(texts),
        fixture.max_seq_len,
        case.emits_token_type_ids(),
    );
    let session = OrtTokenizedScorerSession::load_from_file(case.descriptor(), &case.model_path)
        .unwrap_or_else(|e| panic!("session load: {e}"));

    let qctx_for = |i: usize| QueryContext {
        query_text: Some(Arc::from(fixture.pairs[i].query.as_str())),
        ..QueryContext::default()
    };

    // End-to-end (tokenize + score) for a single query's rerank window: one
    // query, W docs — the shape a real rerank request takes.
    for window in [10usize, 25, 50] {
        let docs: Vec<DocHandle> = (0..window as u32).map(DocHandle).collect();
        // Warm-up (first ort run pays session/graph init).
        let _ = extractor.extract_batch(&docs, &qctx_for(0));
        let mut samples = Vec::new();
        for rep in 0..30 {
            let qctx = qctx_for(rep % fixture.pairs.len());
            let start = Instant::now();
            let batch = extractor.extract_batch(&docs, &qctx).expect("extract");
            let scores = session.score(&batch).expect("score");
            samples.push(start.elapsed().as_secs_f64() * 1000.0);
            assert_eq!(scores.len(), window);
        }
        samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mean: f64 = samples.iter().sum::<f64>() / samples.len() as f64;
        println!(
            "{label} rerank window {window:>3}: mean {mean:8.2} ms  p50 {:8.2} ms  p95 {:8.2} ms  (tokenize+infer, 1×{window})",
            samples[samples.len() / 2],
            samples[(samples.len() as f64 * 0.95) as usize],
        );
    }
}
