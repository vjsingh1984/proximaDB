//! `TokenizedDocFeatureExtractor` — produces a [`TokenizedBatch`] from a
//! slice of `DocHandle`s. R-5b.1.1.
//!
//! Twin of [`DocFeatureExtractor`](crate::doc_feature_extractor::DocFeatureExtractor)
//! adapted for tokenized cross-encoders. Two key differences:
//!
//! 1. **Batch-oriented**: tokenization is naturally batched — the
//!    HuggingFace `tokenizers` crate exposes `encode_batch` for
//!    parallel encoding, and BERT inference batches are
//!    rectangular tensors. Per-doc extraction would force a tokenize
//!    call per doc, defeating the throughput rationale.
//! 2. **Output shape**: returns a `TokenizedBatch` rather than a
//!    `Vec<f32>` per doc, because the underlying inference shape is
//!    three int64 tensors, not one float vector.
//!
//! Production implementations (R-5b.1.2 follow-up) will pull
//! `(query_text, doc_text)` pairs from attribute storage and run them
//! through a shared `Arc<tokenizers::Tokenizer>` to produce
//! input_ids / attention_mask / token_type_ids. v1 ships the trait +
//! a Noop impl + a closure impl so the pipeline can be exercised
//! against synthetic data.

use proximadb_rank_core::{DocHandle, QueryContext, RankResult};

use crate::tokenized_scorer_session::TokenizedBatch;

/// Batch-oriented tokenized feature producer.
///
/// Contract: `extract_batch(docs)` must return a `TokenizedBatch` with
/// `batch_size() == docs.len()`. Row `i` in the batch corresponds to
/// `docs[i]`. Implementations that need a different shape (e.g.
/// summary-features extraction that ignores docs) belong elsewhere.
pub trait TokenizedDocFeatureExtractor: Send + Sync {
    /// `qctx` carries per-request state — specifically `query_text`
    /// for the BertPair extractor (R-5b.1.3). Extractors that don't
    /// need the query (e.g. Noop fixtures) ignore it.
    fn extract_batch(&self, docs: &[DocHandle], qctx: &QueryContext) -> RankResult<TokenizedBatch>;
}

/// Test/null fixture — emits a `TokenizedBatch` with one zero-row per
/// doc. Useful for wiring tests that exercise the scorer pipeline
/// against mock sessions without setting up real tokenizer state.
pub struct NoopTokenizedDocFeatureExtractor {
    pub seq_len: usize,
}

impl Default for NoopTokenizedDocFeatureExtractor {
    fn default() -> Self {
        Self { seq_len: 1 }
    }
}

impl TokenizedDocFeatureExtractor for NoopTokenizedDocFeatureExtractor {
    fn extract_batch(
        &self,
        docs: &[DocHandle],
        _qctx: &QueryContext,
    ) -> RankResult<TokenizedBatch> {
        let n = docs.len();
        Ok(TokenizedBatch::new(
            (0..n).map(|_| vec![0i64; self.seq_len]).collect(),
            (0..n).map(|_| vec![0i64; self.seq_len]).collect(),
        ))
    }
}

/// Closure-based extractor for tests. Captures arbitrary state and
/// hands it back per-doc via a user-supplied callback that builds the
/// batch. Useful for exercising the second-phase pipeline against
/// canned token sequences.
type ExtractFn =
    Box<dyn Fn(&[DocHandle], &QueryContext) -> RankResult<TokenizedBatch> + Send + Sync>;

pub struct FnTokenizedDocFeatureExtractor {
    f: ExtractFn,
}

impl FnTokenizedDocFeatureExtractor {
    pub fn new<F>(f: F) -> Self
    where
        F: Fn(&[DocHandle], &QueryContext) -> RankResult<TokenizedBatch> + Send + Sync + 'static,
    {
        Self { f: Box::new(f) }
    }
}

impl TokenizedDocFeatureExtractor for FnTokenizedDocFeatureExtractor {
    fn extract_batch(&self, docs: &[DocHandle], qctx: &QueryContext) -> RankResult<TokenizedBatch> {
        (self.f)(docs, qctx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_extractor_returns_one_zero_row_per_doc() {
        let e = NoopTokenizedDocFeatureExtractor::default();
        let b = e
            .extract_batch(
                &[DocHandle(1), DocHandle(2), DocHandle(3)],
                &QueryContext::default(),
            )
            .unwrap();
        assert_eq!(b.batch_size(), 3);
        assert_eq!(b.seq_len(), 1);
        for row in &b.input_ids {
            assert_eq!(row, &vec![0i64]);
        }
        for row in &b.attention_mask {
            assert_eq!(row, &vec![0i64]);
        }
        assert!(b.token_type_ids.is_none());
    }

    #[test]
    fn noop_extractor_honors_configured_seq_len() {
        let e = NoopTokenizedDocFeatureExtractor { seq_len: 8 };
        let b = e
            .extract_batch(&[DocHandle(42)], &QueryContext::default())
            .unwrap();
        assert_eq!(b.seq_len(), 8);
        assert_eq!(b.input_ids[0].len(), 8);
    }

    #[test]
    fn noop_extractor_handles_empty_doc_list() {
        let e = NoopTokenizedDocFeatureExtractor::default();
        let b = e.extract_batch(&[], &QueryContext::default()).unwrap();
        assert_eq!(b.batch_size(), 0);
        assert!(b.validate_rectangular().is_ok());
    }

    #[test]
    fn fn_extractor_dispatches_to_closure() {
        // Closure that builds a deterministic batch from doc ids —
        // input_ids[i] = [doc.0 as i64; 2], attention_mask all 1s.
        let e = FnTokenizedDocFeatureExtractor::new(|docs, _qctx| {
            Ok(TokenizedBatch::new(
                docs.iter().map(|d| vec![d.0 as i64; 2]).collect(),
                docs.iter().map(|_| vec![1i64; 2]).collect(),
            ))
        });
        let b = e
            .extract_batch(&[DocHandle(10), DocHandle(20)], &QueryContext::default())
            .unwrap();
        assert_eq!(b.batch_size(), 2);
        assert_eq!(b.input_ids[0], vec![10, 10]);
        assert_eq!(b.input_ids[1], vec![20, 20]);
    }
}
