//! Per-document feature extraction for batched second-phase scoring.
//!
//! The cross-encoder scorer needs an input tensor per candidate doc
//! (e.g. tokenized `[query, doc_summary]` for a BERT-style reranker).
//! `DocFeatureExtractor` is the abstraction between "the pipeline knows
//! DocHandles" and "the scorer needs feature vectors". Production
//! implementations read attribute fields, tokenize via a shared
//! `tokenizers::Tokenizer`, and return input ids. Tests use closure-
//! backed mocks.

use proximadb_rank_core::{DocHandle, RankResult};

/// Produce one input feature vector per candidate doc.
///
/// `extract` is called once per doc per second-phase invocation.
/// Output length must equal the model's expected input width; the
/// caller (`OnnxSecondPhaseScorer`) doesn't validate that here, so
/// extractors are responsible for matching the model contract.
pub trait DocFeatureExtractor: Send + Sync {
    fn extract(&self, doc: DocHandle) -> RankResult<Vec<f32>>;
}

/// No-op extractor — returns an empty vector for every doc. Useful as
/// a default when the scorer doesn't actually need per-doc features
/// (e.g. a `ConstantMultiplierSecondPhaseScorer`-style mock that just
/// scales by an existing score).
pub struct NoopDocFeatureExtractor;

impl DocFeatureExtractor for NoopDocFeatureExtractor {
    fn extract(&self, _doc: DocHandle) -> RankResult<Vec<f32>> {
        Ok(Vec::new())
    }
}

/// Closure-backed extractor for tests + flexible production
/// implementations that want to skip the trait-impl boilerplate. The
/// closure must be `Fn` (called from multiple worker threads) and
/// `Send + Sync` (held behind `Arc` in `RankServices`).
pub struct FnDocFeatureExtractor<F>
where
    F: Fn(DocHandle) -> RankResult<Vec<f32>> + Send + Sync,
{
    f: F,
}

impl<F> FnDocFeatureExtractor<F>
where
    F: Fn(DocHandle) -> RankResult<Vec<f32>> + Send + Sync,
{
    pub fn new(f: F) -> Self {
        Self { f }
    }
}

impl<F> DocFeatureExtractor for FnDocFeatureExtractor<F>
where
    F: Fn(DocHandle) -> RankResult<Vec<f32>> + Send + Sync,
{
    fn extract(&self, doc: DocHandle) -> RankResult<Vec<f32>> {
        (self.f)(doc)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noop_extractor_returns_empty_vec() {
        let e = NoopDocFeatureExtractor;
        let v = e.extract(DocHandle(42)).unwrap();
        assert!(v.is_empty());
    }

    #[test]
    fn fn_extractor_calls_closure_per_doc() {
        let e = FnDocFeatureExtractor::new(|doc: DocHandle| Ok(vec![doc.0 as f32, 0.0, 1.0]));
        let v = e.extract(DocHandle(7)).unwrap();
        assert_eq!(v, vec![7.0, 0.0, 1.0]);
        let v2 = e.extract(DocHandle(99)).unwrap();
        assert_eq!(v2, vec![99.0, 0.0, 1.0]);
    }

    #[test]
    fn fn_extractor_propagates_errors() {
        use proximadb_rank_core::RankError;
        let e = FnDocFeatureExtractor::new(|_doc: DocHandle| {
            Err(RankError::ModelInference {
                model_id: "test".into(),
                reason: "boom".into(),
            })
        });
        match e.extract(DocHandle(0)) {
            Err(RankError::ModelInference { reason, .. }) => assert_eq!(reason, "boom"),
            Err(_) => panic!("expected ModelInference"),
            Ok(_) => panic!("expected error"),
        }
    }
}
