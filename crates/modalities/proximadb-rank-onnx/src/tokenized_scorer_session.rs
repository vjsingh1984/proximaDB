//! [`TokenizedScorerSession`] — int64 token-tensor twin of
//! [`ScorerSession`](crate::scorer_session::ScorerSession). R-5b.1.
//!
//! Closes the production gap that limited the real-onnx surface to
//! pre-encoded-feature models. The canonical reranker family in
//! production is BERT-style cross-encoders (e.g.
//! `cross-encoder/ms-marco-MiniLM-L-12-v2`,
//! `BAAI/bge-reranker-large`), and those models take three int64
//! tensors — `input_ids`, `attention_mask`, `token_type_ids` — not a
//! single float tensor. Sharing the existing [`ScorerSession`] trait
//! would have widened its signature for every implementor; a parallel
//! trait keeps both surfaces narrow.
//!
//! Threading & lifecycle: same as [`ScorerSession`]. Sessions are
//! immutable post-load (`Send + Sync + 'static`), live behind `Arc` in
//! the model cache, and their `score` call is allowed to take an
//! internal `&mut` (via `Mutex<…>`) — see [`OrtTokenizedScorerSession`].
//!
//! Where tokenization happens (deliberately upstream):
//! The pre-encoding step (turn `(query_text, doc_text)` into the three
//! int64 tensors) is a `DocFeatureExtractor`-level concern, not a
//! `ScorerSession` concern. The session takes already-tokenized
//! batches so the trait stays inference-only — pure, side-effect-free,
//! easy to mock. The [`crate::doc_feature_extractor`] crate is the
//! natural home for a `BertPairTokenizingExtractor` follow-up; the
//! `tokenizers` crate is already pulled in by `proximadb-embedding`.

use crate::descriptor::ModelDescriptor;
use proximadb_rank_core::{RankError, RankResult};
use std::sync::atomic::{AtomicU64, Ordering};

/// One batched tokenization input for a tokenized scorer.
///
/// Each row is one (query, doc) pair already encoded. Sequence length
/// must be uniform across rows (the session builds a rectangular int64
/// tensor); callers should have padded shorter pairs to the model's max
/// length and truncated longer ones at tokenization time.
///
/// `token_type_ids` is optional because some models (e.g.
/// MiniLM-derived) don't take a segment-id input. When absent, the
/// session simply doesn't bind that input slot — the descriptor's
/// `input_spec` is the source of truth for which slots are required.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TokenizedBatch {
    pub input_ids: Vec<Vec<i64>>,
    pub attention_mask: Vec<Vec<i64>>,
    pub token_type_ids: Option<Vec<Vec<i64>>>,
}

impl TokenizedBatch {
    /// Build a single-input batch (input_ids + attention_mask only).
    /// Convenience for models that don't use token_type_ids.
    pub fn new(input_ids: Vec<Vec<i64>>, attention_mask: Vec<Vec<i64>>) -> Self {
        Self {
            input_ids,
            attention_mask,
            token_type_ids: None,
        }
    }

    /// Number of (query, doc) pairs in this batch.
    pub fn batch_size(&self) -> usize {
        self.input_ids.len()
    }

    /// Sequence length of the first row. Returns 0 if the batch is empty.
    /// Callers that require uniform width should `validate_rectangular`
    /// first.
    pub fn seq_len(&self) -> usize {
        self.input_ids.first().map(|r| r.len()).unwrap_or(0)
    }

    /// Validate that all per-tensor row widths match across the batch
    /// (the session will build a rectangular int64 tensor; ragged input
    /// fails inference).
    pub fn validate_rectangular(&self) -> Result<(), String> {
        if self.input_ids.is_empty() {
            return Ok(());
        }
        let width = self.input_ids[0].len();
        for (i, row) in self.input_ids.iter().enumerate() {
            if row.len() != width {
                return Err(format!(
                    "input_ids row {i} has width {} but row 0 has width {width}",
                    row.len()
                ));
            }
        }
        if self.attention_mask.len() != self.input_ids.len() {
            return Err(format!(
                "attention_mask has {} rows but input_ids has {}",
                self.attention_mask.len(),
                self.input_ids.len()
            ));
        }
        for (i, row) in self.attention_mask.iter().enumerate() {
            if row.len() != width {
                return Err(format!(
                    "attention_mask row {i} has width {} but input_ids row 0 has width {width}",
                    row.len()
                ));
            }
        }
        if let Some(tti) = &self.token_type_ids {
            if tti.len() != self.input_ids.len() {
                return Err(format!(
                    "token_type_ids has {} rows but input_ids has {}",
                    tti.len(),
                    self.input_ids.len()
                ));
            }
            for (i, row) in tti.iter().enumerate() {
                if row.len() != width {
                    return Err(format!(
                        "token_type_ids row {i} has width {} but input_ids row 0 has width {width}",
                        row.len()
                    ));
                }
            }
        }
        Ok(())
    }
}

/// Tokenized scorer session. One f32 score per row, same as
/// [`ScorerSession`](crate::scorer_session::ScorerSession) — the
/// difference is the input shape.
pub trait TokenizedScorerSession: Send + Sync + 'static {
    fn descriptor(&self) -> &ModelDescriptor;

    fn memory_bytes(&self) -> usize {
        self.descriptor().estimated_memory_bytes()
    }

    /// Score the batch. The implementation is responsible for binding
    /// `batch.input_ids` to the first declared input slot in
    /// `descriptor.input_spec`, `batch.attention_mask` to the second,
    /// and (when `batch.token_type_ids` is Some + a third slot exists)
    /// the third. Returns one f32 score per row.
    fn score(&self, batch: &TokenizedBatch) -> RankResult<Vec<f32>>;
}

/// Test-only [`TokenizedScorerSession`] returning deterministic scores.
/// Sibling to [`MockScorerSession`](crate::scorer_session::MockScorerSession);
/// kept symmetric so cache + batch tests can swap one for the other.
type TokenizedScoringFn = Box<dyn Fn(&TokenizedBatch) -> Vec<f32> + Send + Sync>;

pub struct MockTokenizedScorerSession {
    descriptor: ModelDescriptor,
    scoring_fn: TokenizedScoringFn,
    call_count: AtomicU64,
    rows_seen: AtomicU64,
}

impl MockTokenizedScorerSession {
    pub fn new<F>(descriptor: ModelDescriptor, scoring_fn: F) -> Self
    where
        F: Fn(&TokenizedBatch) -> Vec<f32> + Send + Sync + 'static,
    {
        Self {
            descriptor,
            scoring_fn: Box::new(scoring_fn),
            call_count: AtomicU64::new(0),
            rows_seen: AtomicU64::new(0),
        }
    }

    /// Shortcut: each row scores 0.0.
    pub fn zeros(descriptor: ModelDescriptor) -> Self {
        Self::new(descriptor, |b| vec![0.0; b.batch_size()])
    }

    /// Shortcut: each row scores `score_per_row` (constant).
    pub fn constant(descriptor: ModelDescriptor, score_per_row: f32) -> Self {
        Self::new(descriptor, move |b| vec![score_per_row; b.batch_size()])
    }

    pub fn call_count(&self) -> u64 {
        self.call_count.load(Ordering::SeqCst)
    }

    pub fn rows_seen(&self) -> u64 {
        self.rows_seen.load(Ordering::SeqCst)
    }
}

impl TokenizedScorerSession for MockTokenizedScorerSession {
    fn descriptor(&self) -> &ModelDescriptor {
        &self.descriptor
    }

    fn score(&self, batch: &TokenizedBatch) -> RankResult<Vec<f32>> {
        if let Err(msg) = batch.validate_rectangular() {
            return Err(RankError::ModelInference {
                model_id: self.descriptor.key.to_string(),
                reason: format!("MockTokenizedScorerSession: ragged batch: {msg}"),
            });
        }
        self.call_count.fetch_add(1, Ordering::SeqCst);
        self.rows_seen
            .fetch_add(batch.batch_size() as u64, Ordering::SeqCst);
        let scores = (self.scoring_fn)(batch);
        debug_assert_eq!(
            scores.len(),
            batch.batch_size(),
            "MockTokenizedScorerSession closure must return one score per row"
        );
        Ok(scores)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{DType, ModelFramework, ModelKey, TensorIoSpec};

    fn descriptor() -> ModelDescriptor {
        ModelDescriptor {
            key: ModelKey::new("bert-cross-encoder", "1"),
            tenant: None,
            uri: "file:///tmp/bce.onnx".into(),
            sha256: [0; 32],
            size_bytes: 2048,
            framework: ModelFramework::Onnx,
            dtype: DType::Fp32,
            input_spec: vec![
                TensorIoSpec {
                    name: "input_ids".into(),
                    shape: vec![None, Some(128)],
                    dtype: DType::Fp32, // descriptor doesn't carry int64 yet — v1 is float
                },
                TensorIoSpec {
                    name: "attention_mask".into(),
                    shape: vec![None, Some(128)],
                    dtype: DType::Fp32,
                },
            ],
            output_spec: vec![TensorIoSpec {
                name: "logits".into(),
                shape: vec![None, Some(1)],
                dtype: DType::Fp32,
            }],
            max_batch_size: 16,
            seq: 0,
            created_at_ms: 0,
        }
    }

    fn batch_of(rows: &[(&[i64], &[i64])]) -> TokenizedBatch {
        TokenizedBatch::new(
            rows.iter().map(|(ids, _)| ids.to_vec()).collect(),
            rows.iter().map(|(_, am)| am.to_vec()).collect(),
        )
    }

    // ---------------- TokenizedBatch shape & validation ----------------

    #[test]
    fn empty_batch_has_zero_batch_size_and_seq_len() {
        let b = TokenizedBatch::default();
        assert_eq!(b.batch_size(), 0);
        assert_eq!(b.seq_len(), 0);
        assert!(b.validate_rectangular().is_ok());
    }

    #[test]
    fn batch_new_constructs_without_token_type_ids() {
        let b = TokenizedBatch::new(vec![vec![1, 2, 3]], vec![vec![1, 1, 1]]);
        assert_eq!(b.batch_size(), 1);
        assert_eq!(b.seq_len(), 3);
        assert!(b.token_type_ids.is_none());
        assert!(b.validate_rectangular().is_ok());
    }

    #[test]
    fn batch_validate_rectangular_rejects_uneven_input_ids() {
        let b = TokenizedBatch::new(
            vec![vec![1, 2, 3], vec![1, 2]],
            vec![vec![1, 1, 1], vec![1, 1]],
        );
        let err = b.validate_rectangular().unwrap_err();
        assert!(err.contains("input_ids row 1"));
        assert!(err.contains("width 2"));
    }

    #[test]
    fn batch_validate_rectangular_rejects_attention_mask_row_count_mismatch() {
        let b = TokenizedBatch::new(vec![vec![1, 2, 3], vec![4, 5, 6]], vec![vec![1, 1, 1]]);
        let err = b.validate_rectangular().unwrap_err();
        assert!(err.contains("attention_mask has 1 rows but input_ids has 2"));
    }

    #[test]
    fn batch_validate_rectangular_rejects_attention_mask_width_mismatch() {
        let b = TokenizedBatch::new(vec![vec![1, 2, 3]], vec![vec![1, 1]]);
        let err = b.validate_rectangular().unwrap_err();
        assert!(err.contains("attention_mask row 0"));
    }

    #[test]
    fn batch_validate_rectangular_checks_token_type_ids_when_present() {
        let mut b = TokenizedBatch::new(vec![vec![1, 2, 3]], vec![vec![1, 1, 1]]);
        b.token_type_ids = Some(vec![vec![0, 0]]); // wrong width
        let err = b.validate_rectangular().unwrap_err();
        assert!(err.contains("token_type_ids row 0"));
    }

    #[test]
    fn batch_validate_rectangular_accepts_token_type_ids_when_shape_matches() {
        let mut b = TokenizedBatch::new(
            vec![vec![1, 2, 3], vec![4, 5, 6]],
            vec![vec![1, 1, 1], vec![1, 1, 1]],
        );
        b.token_type_ids = Some(vec![vec![0, 0, 1], vec![0, 0, 1]]);
        assert!(b.validate_rectangular().is_ok());
    }

    // ---------------- MockTokenizedScorerSession ----------------

    #[test]
    fn mock_zeros_returns_zero_scores() {
        let s = MockTokenizedScorerSession::zeros(descriptor());
        let b = batch_of(&[(&[1, 2, 3], &[1, 1, 1]), (&[4, 5, 6], &[1, 1, 1])]);
        let out = s.score(&b).unwrap();
        assert_eq!(out, vec![0.0, 0.0]);
        assert_eq!(s.call_count(), 1);
        assert_eq!(s.rows_seen(), 2);
    }

    #[test]
    fn mock_constant_returns_same_score_per_row() {
        let s = MockTokenizedScorerSession::constant(descriptor(), 0.75);
        let b = batch_of(&[
            (&[1, 2, 3], &[1, 1, 1]),
            (&[4, 5, 6], &[1, 1, 1]),
            (&[7, 8, 9], &[1, 1, 1]),
        ]);
        let out = s.score(&b).unwrap();
        assert_eq!(out, vec![0.75; 3]);
    }

    #[test]
    fn mock_call_count_accumulates_across_calls() {
        let s = MockTokenizedScorerSession::zeros(descriptor());
        s.score(&batch_of(&[(&[1], &[1])])).unwrap();
        s.score(&batch_of(&[(&[1], &[1]), (&[2], &[1])])).unwrap();
        assert_eq!(s.call_count(), 2);
        assert_eq!(s.rows_seen(), 3);
    }

    #[test]
    fn mock_with_custom_closure_uses_input_ids() {
        // Closure: score = sum(input_ids[i]) as f32. Verifies the
        // session passes the actual batch through to the closure rather
        // than dropping it.
        let s = MockTokenizedScorerSession::new(descriptor(), |b| {
            b.input_ids
                .iter()
                .map(|row| row.iter().sum::<i64>() as f32)
                .collect()
        });
        let b = batch_of(&[(&[1, 2, 3], &[1, 1, 1]), (&[10, 20, 30], &[1, 1, 1])]);
        let out = s.score(&b).unwrap();
        assert_eq!(out, vec![6.0, 60.0]);
    }

    #[test]
    fn mock_rejects_ragged_batch_with_model_inference_error() {
        let s = MockTokenizedScorerSession::zeros(descriptor());
        let b = TokenizedBatch::new(vec![vec![1, 2], vec![3]], vec![vec![1, 1], vec![1]]);
        match s.score(&b) {
            Err(RankError::ModelInference { reason, .. }) => {
                assert!(reason.contains("ragged batch"));
            }
            other => panic!("expected ModelInference, got {other:?}"),
        }
        // Failed call doesn't count towards call_count (validation runs before).
        assert_eq!(s.call_count(), 0);
    }

    #[test]
    fn mock_memory_bytes_defaults_to_descriptor_estimate() {
        let s = MockTokenizedScorerSession::zeros(descriptor());
        assert_eq!(s.memory_bytes(), 2048);
    }

    #[test]
    fn tokenized_scorer_session_is_object_safe() {
        // Compile-time check that the trait is dyn-compatible — the
        // model cache + batched-scorer infrastructure will hold
        // `Arc<dyn TokenizedScorerSession>`.
        fn _accepts_dyn(_s: &dyn TokenizedScorerSession) {}
        let s = MockTokenizedScorerSession::zeros(descriptor());
        _accepts_dyn(&s);
    }
}
