//! `TokenizedBatchedScorer` — int64-token twin of [`BatchedScorer`]. R-5b.1.1.
//!
//! Same job — chunk a full batch of (query, doc) inputs by the
//! session's `max_batch_size` and call into the underlying
//! `TokenizedScorerSession` once per chunk. Different input shape —
//! tokenized batches instead of flat float vectors.
//!
//! The two surfaces live side-by-side rather than being merged into a
//! sum-typed trait because: (a) a model is either tokenized OR
//! pre-encoded, never both; (b) chunking semantics are different (we
//! slice rows from a `TokenizedBatch` rather than chunking
//! `Vec<Vec<f32>>`); (c) sum-typed inputs would force every consumer
//! to match on the variant on every call.
//!
//! v1 holds the session directly (`Arc<dyn TokenizedScorerSession>`)
//! rather than going through `OnnxModelCache` — the cache only knows
//! about `ScorerSession` and adding parallel tokenized methods would
//! double its surface for marginal v1 payoff. R-5b.1.1.x will fold
//! tokenized sessions into the cache once we have multiple production
//! models contending for memory.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` §4.3.

use std::sync::Arc;

use proximadb_rank_core::{DocHandle, RankError, RankResult};

use crate::batched_scorer::BatchOutput;
use crate::tokenized_scorer_session::{TokenizedBatch, TokenizedScorerSession};

/// Input to one tokenized-batched scoring pass.
///
/// `docs.len()` MUST equal `batch.batch_size()`. The scorer validates
/// this on entry and rejects mismatches before paying any inference
/// cost.
#[derive(Debug, Clone)]
pub struct TokenizedBatchInput {
    pub docs: Vec<DocHandle>,
    pub batch: TokenizedBatch,
}

impl TokenizedBatchInput {
    pub fn new(docs: Vec<DocHandle>, batch: TokenizedBatch) -> Self {
        Self { docs, batch }
    }
    pub fn len(&self) -> usize {
        self.docs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}

/// Trait for tokenized batched scorers. Returns the same `BatchOutput`
/// shape as [`BatchedScorer`] so downstream consumers (`SecondPhaseScorer`
/// adapters) can map (DocHandle, f32) pairs back to `ScoredHit`s with
/// no shape gymnastics.
pub trait TokenizedBatchedScorer: Send + Sync {
    fn score_batch(&self, input: TokenizedBatchInput) -> RankResult<BatchOutput>;
}

/// Adapter that delegates to an `Arc<dyn TokenizedScorerSession>`,
/// chunking the input batch by the session's `max_batch_size`.
pub struct OnnxTokenizedBatchedScorer {
    session: Arc<dyn TokenizedScorerSession>,
}

impl OnnxTokenizedBatchedScorer {
    pub fn new(session: Arc<dyn TokenizedScorerSession>) -> Self {
        Self { session }
    }
    pub fn session(&self) -> &dyn TokenizedScorerSession {
        self.session.as_ref()
    }
}

impl TokenizedBatchedScorer for OnnxTokenizedBatchedScorer {
    fn score_batch(&self, input: TokenizedBatchInput) -> RankResult<BatchOutput> {
        let model_id = self.session.descriptor().key.to_string();
        if input.docs.len() != input.batch.batch_size() {
            return Err(RankError::ModelInference {
                model_id,
                reason: format!(
                    "TokenizedBatchInput.docs.len()={} != batch.batch_size()={}",
                    input.docs.len(),
                    input.batch.batch_size()
                ),
            });
        }
        if input.is_empty() {
            return Ok(BatchOutput::empty());
        }
        let max = self.session.descriptor().max_batch_size.max(1);
        let TokenizedBatchInput { docs, batch } = input;

        // Slice the TokenizedBatch into `max`-row chunks. Each chunk
        // shares the same per-input-tensor structure as the parent
        // (input_ids / attention_mask / token_type_ids); chunking is
        // just a row-window across all three slot-tensors.
        let total = docs.len();
        let mut all_scores = Vec::with_capacity(total);
        let mut start = 0usize;
        while start < total {
            let end = (start + max).min(total);
            let chunk = slice_tokenized_batch(&batch, start, end);
            let chunk_size = end - start;
            let scores = self.session.score(&chunk).map_err(|e| match e {
                RankError::ModelInference { .. } => e,
                other => RankError::ModelInference {
                    model_id: model_id.clone(),
                    reason: other.to_string(),
                },
            })?;
            if scores.len() != chunk_size {
                return Err(RankError::ModelInference {
                    model_id: model_id.clone(),
                    reason: format!(
                        "session returned {} scores for {} rows",
                        scores.len(),
                        chunk_size
                    ),
                });
            }
            all_scores.extend(scores);
            start = end;
        }
        Ok(BatchOutput {
            scores: docs.into_iter().zip(all_scores).collect(),
        })
    }
}

/// Build a `TokenizedBatch` containing rows `[start, end)` of `parent`.
/// Clones the row vectors — for production hot paths a future refactor
/// could swap `Vec<Vec<i64>>` for an `Arc<[Arc<[i64]>]>` to chunk
/// without cloning, but the per-row vectors are already small (≤ 512
/// tokens typically) so this isn't a meaningful cost for v1.
fn slice_tokenized_batch(parent: &TokenizedBatch, start: usize, end: usize) -> TokenizedBatch {
    TokenizedBatch {
        input_ids: parent.input_ids[start..end].to_vec(),
        attention_mask: parent.attention_mask[start..end].to_vec(),
        token_type_ids: parent
            .token_type_ids
            .as_ref()
            .map(|tti| tti[start..end].to_vec()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{DType, ModelDescriptor, ModelFramework, ModelKey};
    use crate::tokenized_scorer_session::MockTokenizedScorerSession;

    fn descriptor(id: &str, max_batch_size: usize) -> ModelDescriptor {
        ModelDescriptor {
            key: ModelKey::new(id, "1"),
            tenant: None,
            uri: format!("file:///tmp/{id}.onnx"),
            sha256: [0; 32],
            size_bytes: 1024,
            framework: ModelFramework::Onnx,
            dtype: DType::Fp32,
            input_spec: vec![],
            output_spec: vec![],
            max_batch_size,
            seq: 0,
            created_at_ms: 0,
        }
    }

    fn make_input(n: usize, seq_len: usize) -> TokenizedBatchInput {
        let docs: Vec<DocHandle> = (0..n as u32).map(DocHandle).collect();
        let input_ids: Vec<Vec<i64>> = (0..n).map(|i| vec![i as i64; seq_len]).collect();
        let attention_mask: Vec<Vec<i64>> = (0..n).map(|_| vec![1; seq_len]).collect();
        TokenizedBatchInput::new(docs, TokenizedBatch::new(input_ids, attention_mask))
    }

    // ---------------- Chunking behavior ----------------

    #[test]
    fn batches_correctly_at_exact_multiple() {
        let mock = Arc::new(MockTokenizedScorerSession::zeros(descriptor("r", 32)));
        let scorer = OnnxTokenizedBatchedScorer::new(mock.clone());
        let out = scorer.score_batch(make_input(64, 8)).unwrap();
        assert_eq!(out.scores.len(), 64);
        assert_eq!(mock.call_count(), 2);
        assert_eq!(mock.rows_seen(), 64);
    }

    #[test]
    fn batches_correctly_with_tail() {
        let mock = Arc::new(MockTokenizedScorerSession::zeros(descriptor("r", 32)));
        let scorer = OnnxTokenizedBatchedScorer::new(mock.clone());
        let out = scorer.score_batch(make_input(100, 8)).unwrap();
        assert_eq!(out.scores.len(), 100);
        assert_eq!(mock.call_count(), 4); // 32 + 32 + 32 + 4
        assert_eq!(mock.rows_seen(), 100);
    }

    #[test]
    fn empty_input_short_circuits_with_no_inference_calls() {
        let mock = Arc::new(MockTokenizedScorerSession::zeros(descriptor("r", 32)));
        let scorer = OnnxTokenizedBatchedScorer::new(mock.clone());
        let out = scorer
            .score_batch(TokenizedBatchInput::new(vec![], TokenizedBatch::default()))
            .unwrap();
        assert!(out.scores.is_empty());
        assert_eq!(mock.call_count(), 0);
    }

    #[test]
    fn batch_size_one_calls_per_doc() {
        let mock = Arc::new(MockTokenizedScorerSession::zeros(descriptor("r", 1)));
        let scorer = OnnxTokenizedBatchedScorer::new(mock.clone());
        scorer.score_batch(make_input(10, 4)).unwrap();
        assert_eq!(mock.call_count(), 10);
    }

    #[test]
    fn rejects_mismatched_docs_and_batch_size() {
        let mock = Arc::new(MockTokenizedScorerSession::zeros(descriptor("r", 32)));
        let scorer = OnnxTokenizedBatchedScorer::new(mock);
        let bad = TokenizedBatchInput::new(vec![DocHandle(0)], TokenizedBatch::default());
        match scorer.score_batch(bad) {
            Err(RankError::ModelInference { reason, .. }) => {
                assert!(reason.contains("docs.len()=1"));
                assert!(reason.contains("batch.batch_size()=0"));
            }
            other => panic!("expected ModelInference, got {other:?}"),
        }
    }

    #[test]
    fn passes_through_docs_in_order_with_correct_scores() {
        // Session that echoes the first token id of each row as the
        // score — lets us verify per-row routing across chunk boundaries.
        let desc = descriptor("echo", 3);
        let echo = Arc::new(MockTokenizedScorerSession::new(desc, |b| {
            b.input_ids.iter().map(|row| row[0] as f32).collect()
        }));
        let scorer = OnnxTokenizedBatchedScorer::new(echo);
        let docs = vec![
            DocHandle(100),
            DocHandle(200),
            DocHandle(300),
            DocHandle(400),
            DocHandle(500),
        ];
        let input_ids: Vec<Vec<i64>> = (0..5).map(|i| vec![(i + 1) * 10, 0, 0]).collect();
        let attention_mask: Vec<Vec<i64>> = (0..5).map(|_| vec![1, 1, 1]).collect();
        let out = scorer
            .score_batch(TokenizedBatchInput::new(
                docs.clone(),
                TokenizedBatch::new(input_ids, attention_mask),
            ))
            .unwrap();
        for (i, (doc, score)) in out.scores.iter().enumerate() {
            assert_eq!(*doc, docs[i]);
            assert!((score - ((i as f32 + 1.0) * 10.0)).abs() < 1e-5);
        }
    }

    #[test]
    fn slice_preserves_token_type_ids_when_present() {
        let mut parent = TokenizedBatch::new(
            vec![vec![1], vec![2], vec![3], vec![4]],
            vec![vec![1], vec![1], vec![1], vec![1]],
        );
        parent.token_type_ids = Some(vec![vec![0], vec![1], vec![0], vec![1]]);
        let slice = slice_tokenized_batch(&parent, 1, 3);
        assert_eq!(slice.input_ids, vec![vec![2], vec![3]]);
        assert_eq!(slice.attention_mask, vec![vec![1], vec![1]]);
        assert_eq!(slice.token_type_ids, Some(vec![vec![1], vec![0]]));
    }

    #[test]
    fn slice_omits_token_type_ids_when_parent_omits() {
        let parent = TokenizedBatch::new(vec![vec![1], vec![2]], vec![vec![1], vec![1]]);
        let slice = slice_tokenized_batch(&parent, 0, 1);
        assert!(slice.token_type_ids.is_none());
    }

    #[test]
    fn propagates_session_error_as_model_inference() {
        // Session returns an empty Vec regardless of input — triggers
        // the row-count guard.
        struct BadSession(ModelDescriptor);
        impl TokenizedScorerSession for BadSession {
            fn descriptor(&self) -> &ModelDescriptor {
                &self.0
            }
            fn score(&self, _batch: &TokenizedBatch) -> RankResult<Vec<f32>> {
                Ok(vec![])
            }
        }
        let bad: Arc<dyn TokenizedScorerSession> = Arc::new(BadSession(descriptor("bad", 4)));
        let scorer = OnnxTokenizedBatchedScorer::new(bad);
        match scorer.score_batch(make_input(8, 4)) {
            Err(RankError::ModelInference { reason, .. }) => {
                assert!(reason.contains("returned 0 scores"));
            }
            other => panic!("expected ModelInference, got {other:?}"),
        }
    }
}
