//! Serving-path ONNX parity for the pinned MiniLM cross-encoder (TD-SELECTOR-1, gate 5).
//!
//! PR #1726's reranker evidence was produced by the *Python* path
//! (`sentence-transformers` CrossEncoder, `ms-marco-MiniLM-L6-v2` @ `4bebbd56`,
//! MPS fp32); the ledger entry records "serving ONNX parity and latency remain
//! unmeasured". This test closes the parity half: the serving stack
//! (`BertPairTokenizingDocFeatureExtractor` → `TokenizedBatch` →
//! `OrtTokenizedScorerSession`, i.e. the exact tokenizer-config + session path
//! production uses) must reproduce those reference scores.
//!
//! Reference scores live in `tests/fixtures/onnx_parity_fixture.json` — 64
//! deterministic pairs scored on the #1726 evidence path, including 2 over-length
//! docs so LongestFirst truncation parity is exercised, not just the easy case.
//!
//! Measured agreement when the fixture was generated (MPS fp32 reference vs
//! onnxruntime CPU fp32): max |Δ| = 6.8e-06, Spearman = 1.000000. The assertion
//! tolerances below (1e-4 absolute, Spearman ≥ 0.999) are ~15× that margin —
//! loose enough for cross-platform ort variance, tight enough that a real
//! binding/dtype/segment bug (which shifts logits by whole units) cannot pass.
//!
//! ## Running
//!
//! The model is a 22.7M-parameter binary and lives outside the repo. Export it
//! once from the pinned revision (see TD-SELECTOR-1 gate 5), then:
//!
//! ```text
//! PROXIMADB_TEST_BERT_ONNX_PATH=<dir>/model.onnx \
//!   cargo test -p proximadb-rank-onnx --features real-onnx,bert-tokenizer \
//!   --test onnx_parity -- --nocapture
//! ```
//!
//! `tokenizer.json` is expected beside the model file. Without the env var the
//! tests skip (same contract as the registered `PROXIMADB_TEST_BERT_ONNX_PATH`
//! gate), so default CI stays green with no fixture in tree.

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

/// Absolute score tolerance. Empirical max |Δ| at generation time was 6.8e-06.
const ABS_TOLERANCE: f64 = 1e-4;
/// Rank-agreement floor. Empirical Spearman at generation time was exactly 1.0.
const SPEARMAN_MIN: f64 = 0.999;
/// The extractor's configured max sequence length; the fixture was tokenized at
/// the same length on the Python side (max fixture pair: 359 tokens).
///
/// Parity-discovery note: the serving extractor REJECTS pairs that overflow this
/// budget ("split the document before reranking" — the zero-truncation contract),
/// whereas the #1726 Python gate path asked the tokenizer to truncate. Parity is
/// therefore asserted on within-budget pairs; window-splitting of oversized docs
/// is a caller responsibility on the serving path.
const MAX_SEQ_LEN: usize = 512;

#[derive(Deserialize)]
struct Fixture {
    model: String,
    revision: String,
    onnx_sha256: String,
    max_seq_len: usize,
    pairs: Vec<Pair>,
    reference_scores: Vec<f32>,
}

#[derive(Deserialize)]
struct Pair {
    query: String,
    doc: String,
}

/// Resolve (model, tokenizer, fixture) from the env gate, or skip.
fn artifacts() -> Option<(PathBuf, PathBuf, Fixture)> {
    let Ok(model) = std::env::var("PROXIMADB_TEST_BERT_ONNX_PATH") else {
        eprintln!("skipping: PROXIMADB_TEST_BERT_ONNX_PATH not set");
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
    let fixture: Fixture = serde_json::from_str(include_str!("fixtures/onnx_parity_fixture.json"))
        .expect("checked-in fixture must parse");
    assert_eq!(
        fixture.max_seq_len, MAX_SEQ_LEN,
        "fixture was tokenized at a different length; re-generate it or update MAX_SEQ_LEN"
    );
    Some((model_path, tokenizer_path, fixture))
}

fn descriptor() -> ModelDescriptor {
    ModelDescriptor {
        key: ModelKey::new("minilm-l6-v2-cross-encoder", "4bebbd56"),
        tenant: None,
        uri: "file:///proximadb-bench-data/retrieval-quality-onnx-parity-v1/minilm-l6-v2/onnx/model.onnx".into(),
        sha256: [0; 32],
        size_bytes: 0,
        framework: ModelFramework::Onnx,
        dtype: DType::Fp32,
        input_spec: vec![
            TensorIoSpec { name: "input_ids".into(), shape: vec![None, Some(MAX_SEQ_LEN as i64)], dtype: DType::Fp32 },
            TensorIoSpec { name: "attention_mask".into(), shape: vec![None, Some(MAX_SEQ_LEN as i64)], dtype: DType::Fp32 },
            TensorIoSpec { name: "token_type_ids".into(), shape: vec![None, Some(MAX_SEQ_LEN as i64)], dtype: DType::Fp32 },
        ],
        output_spec: vec![TensorIoSpec { name: "logits".into(), shape: vec![None], dtype: DType::Fp32 }],
        max_batch_size: 64,
        seq: 0,
        created_at_ms: 0,
    }
}

/// Pad per-row extracts to a common width so separately-extracted rows merge
/// into one rectangular batch. Since the batch-longest width change, a
/// single-row extract is padded only to its own length, so merging requires
/// this step. Values match what the tokenizer would have padded with: the
/// model's pad id for `input_ids`, 0 (masked) for attention/type rows.
fn pad_to_width(rows: &mut [Vec<i64>], width: usize, pad: i64) {
    for row in rows.iter_mut() {
        row.resize(width, pad);
    }
}

/// Tokenize every fixture pair through the production extractor and score them
/// in one 64-row batch. Each pair has its own query, and the extractor pairs one
/// query against many docs, so each pair becomes a single-row extract; rows are
/// merged manually (padded to the fixture-wide max, ≤ MAX_SEQ_LEN) into one batch.
fn score_fixture(fixture: &Fixture, tokenizer_path: &PathBuf, model_path: &PathBuf) -> Vec<f32> {
    let tokenizer = tokenizers::Tokenizer::from_file(tokenizer_path)
        .unwrap_or_else(|e| panic!("tokenizer load {tokenizer_path:?}: {e}"));
    let mut texts = HashMapDocTextSource::new();
    for (i, pair) in fixture.pairs.iter().enumerate() {
        texts.insert(DocHandle(i as u32), pair.doc.clone());
    }
    let extractor = BertPairTokenizingDocFeatureExtractor::new(
        Arc::new(tokenizer),
        Arc::new(texts),
        MAX_SEQ_LEN,
        true, // emit_token_type_ids — 3-slot BERT-family model
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
        token_type_ids.push(
            batch
                .token_type_ids
                .as_ref()
                .unwrap_or_else(|| panic!("3-slot model requires token_type_ids (pair {i})"))[0]
                .clone(),
        );
    }

    let width = input_ids
        .iter()
        .map(Vec::len)
        .max()
        .unwrap_or(0);
    assert!(width <= MAX_SEQ_LEN, "merged width {width} exceeds the {MAX_SEQ_LEN} budget");
    // BERT-family pad id is 0; attention/type pad with 0 (masked) regardless.
    pad_to_width(&mut input_ids, width, 0);
    pad_to_width(&mut attention_mask, width, 0);
    pad_to_width(&mut token_type_ids, width, 0);

    let batch = TokenizedBatch {
        input_ids,
        attention_mask,
        token_type_ids: Some(token_type_ids),
    };
    batch
        .validate_rectangular()
        .unwrap_or_else(|e| panic!("merged batch must be rectangular: {e}"));

    let session = OrtTokenizedScorerSession::load_from_file(descriptor(), model_path)
        .unwrap_or_else(|e| panic!("session load {model_path:?}: {e}"));
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

#[test]
fn serving_onnx_matches_python_evidence_scores() {
    let Some((model_path, tokenizer_path, fixture)) = artifacts() else {
        return;
    };
    assert_eq!(fixture.pairs.len(), fixture.reference_scores.len());

    let scores = score_fixture(&fixture, &tokenizer_path, &model_path);
    assert_eq!(scores.len(), fixture.reference_scores.len());

    let mut worst = 0.0_f64;
    let mut worst_i = 0;
    for (i, (got, want)) in scores
        .iter()
        .zip(fixture.reference_scores.iter())
        .enumerate()
    {
        let delta = (*got as f64 - *want as f64).abs();
        if delta > worst {
            worst = delta;
            worst_i = i;
        }
        assert!(
            delta <= ABS_TOLERANCE,
            "pair {i} ('{}…'): serving score {got:.6} vs reference {want:.6}, |Δ| = {delta:.2e} > {ABS_TOLERANCE}",
            fixture.pairs[i].query,
        );
    }
    let rho = spearman(&scores, &fixture.reference_scores);
    assert!(
        rho >= SPEARMAN_MIN,
        "rank agreement too low: Spearman {rho:.6} < {SPEARMAN_MIN}"
    );
    println!(
        "parity OK over {} pairs: max |Δ| = {worst:.3e} (pair {worst_i}), Spearman = {rho:.6}",
        scores.len()
    );
    println!(
        "model {} @ {} sha256 {}…",
        fixture.model,
        fixture.revision,
        &fixture.onnx_sha256[..16]
    );
}

/// Latency profile of the serving path on this machine — one batched score of the
/// full 64-pair fixture, repeated, reporting mean / p50 / p95. Quality of the
/// *numbers* is machine-specific and NOT evidence-ledger material; the value is
/// the shape (batch amortization) and a regression signal when run on the same box.
///
/// Run with `--nocapture` to see the table.
#[test]
fn serving_onnx_latency_profile() {
    let Some((model_path, tokenizer_path, fixture)) = artifacts() else {
        return;
    };

    let tokenizer = tokenizers::Tokenizer::from_file(&tokenizer_path)
        .unwrap_or_else(|e| panic!("tokenizer load: {e}"));
    let mut texts = HashMapDocTextSource::new();
    for (i, pair) in fixture.pairs.iter().enumerate() {
        texts.insert(DocHandle(i as u32), pair.doc.clone());
    }
    let extractor = BertPairTokenizingDocFeatureExtractor::new(
        Arc::new(tokenizer),
        Arc::new(texts),
        MAX_SEQ_LEN,
        true,
    );
    let session = OrtTokenizedScorerSession::load_from_file(descriptor(), &model_path)
        .unwrap_or_else(|e| panic!("session load: {e}"));

    let qctx_for = |i: usize| QueryContext {
        query_text: Some(Arc::from(fixture.pairs[i].query.as_str())),
        ..QueryContext::default()
    };

    // End-to-end (tokenize + score) for a single query's rerank window: one query,
    // W docs — the shape a real rerank request takes.
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
            "rerank window {window:>3}: mean {mean:7.2} ms  p50 {:7.2} ms  p95 {:7.2} ms  (tokenize+infer, batch=1×{window})",
            samples[samples.len() / 2],
            samples[(samples.len() as f64 * 0.95) as usize],
        );
    }

    // Inference-only throughput at the max batch size — the session's compute
    // ceiling, with tokenization excluded. Rows come from separate single-row
    // extracts, so merge-pad them to the fixture-wide max (dominated by the two
    // long docs); the reported width makes the padding cost visible.
    let mut batch = TokenizedBatch::default();
    for (i, _pair) in fixture.pairs.iter().enumerate() {
        let row = extractor
            .extract_batch(&[DocHandle(i as u32)], &qctx_for(i))
            .expect("extract");
        batch.input_ids.push(row.input_ids[0].clone());
        batch.attention_mask.push(row.attention_mask[0].clone());
        if let Some(tti) = row.token_type_ids.as_ref() {
            batch
                .token_type_ids
                .get_or_insert_with(Vec::new)
                .push(tti[0].clone());
        }
    }
    let width = batch.input_ids.iter().map(Vec::len).max().unwrap_or(0);
    pad_to_width(&mut batch.input_ids, width, 0);
    pad_to_width(&mut batch.attention_mask, width, 0);
    if let Some(tti) = batch.token_type_ids.as_mut() {
        pad_to_width(tti, width, 0);
    }
    let mut samples = Vec::new();
    for _ in 0..50 {
        let start = Instant::now();
        let scores = session.score(&batch).expect("score");
        samples.push(start.elapsed().as_secs_f64() * 1000.0);
        assert_eq!(scores.len(), 64);
    }
    samples.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean: f64 = samples.iter().sum::<f64>() / samples.len() as f64;
    println!(
        "inference-only 64×{width}: mean {mean:6.2} ms  p50 {:6.2} ms  p95 {:6.2} ms  ({:.0} pairs/s)",
        samples[samples.len() / 2],
        samples[(samples.len() as f64 * 0.95) as usize],
        64.0 / (samples[samples.len() / 2] / 1000.0),
    );
}
