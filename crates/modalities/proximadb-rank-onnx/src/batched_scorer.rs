//! `BatchedScorer` — cross-encoder-style scoring with deterministic chunking.
//!
//! Cross-encoders must batch: per-doc inference is too slow. `BatchedScorer`
//! takes a full `BatchInput` (all docs to score), chunks rows by the
//! session's `max_batch_size`, calls `ScorerSession::score` once per chunk,
//! and returns one f32 per input doc.
//!
//! `RankPipeline::run_second_phase` (R-6 wiring) calls a `BatchedScorer`
//! after the first phase produces top-K candidates, then merges the
//! returned scores back into the second-phase output slots.

use crate::model_cache::ScorerToken;
use proximadb_rank_core::{DocHandle, RankError, RankResult};

/// Input to one second-phase scoring pass.
#[derive(Debug, Clone)]
pub struct BatchInput {
    pub docs: Vec<DocHandle>,
    /// One row per doc. v1 uses a flat `Vec<f32>` so the test mock can
    /// produce deterministic scores; R-5b's `ort` impl tokenizes upstream
    /// of this call.
    pub rows: Vec<Vec<f32>>,
}

impl BatchInput {
    pub fn new(docs: Vec<DocHandle>, rows: Vec<Vec<f32>>) -> Self {
        Self { docs, rows }
    }
    pub fn len(&self) -> usize {
        self.docs.len()
    }
    pub fn is_empty(&self) -> bool {
        self.docs.is_empty()
    }
}

/// Output of one second-phase scoring pass.
#[derive(Debug, Clone)]
pub struct BatchOutput {
    pub scores: Vec<(DocHandle, f32)>,
}

impl BatchOutput {
    pub fn empty() -> Self {
        Self { scores: Vec::new() }
    }
}

/// Trait for any batched scorer (currently just ONNX; future variants
/// could call out to remote rerank APIs like Cohere if we ever wire
/// them as a `BatchedScorer` adapter).
pub trait BatchedScorer: Send + Sync {
    fn score_batch(&self, batch: BatchInput) -> RankResult<BatchOutput>;
}

/// Adapter that delegates to a `ScorerToken`, chunking inputs by the
/// underlying session's `max_batch_size`.
pub struct OnnxBatchedScorer {
    token: ScorerToken,
}

impl OnnxBatchedScorer {
    pub fn new(token: ScorerToken) -> Self {
        Self { token }
    }
    pub fn token(&self) -> &ScorerToken {
        &self.token
    }
}

impl BatchedScorer for OnnxBatchedScorer {
    fn score_batch(&self, batch: BatchInput) -> RankResult<BatchOutput> {
        if batch.docs.len() != batch.rows.len() {
            return Err(RankError::ModelInference {
                model_id: self.token.descriptor().key.to_string(),
                reason: format!(
                    "BatchInput.docs.len()={} != BatchInput.rows.len()={}",
                    batch.docs.len(),
                    batch.rows.len()
                ),
            });
        }
        if batch.is_empty() {
            return Ok(BatchOutput::empty());
        }
        let max = self.token.descriptor().max_batch_size.max(1);
        let mut all_scores = Vec::with_capacity(batch.docs.len());
        for chunk in batch.rows.chunks(max) {
            let scores = self.token.score(chunk).map_err(|e| match e {
                // Wrap nested RankError so the caller sees ModelInference at
                // the BatchedScorer boundary.
                RankError::ModelInference { .. } => e,
                other => RankError::ModelInference {
                    model_id: self.token.descriptor().key.to_string(),
                    reason: other.to_string(),
                },
            })?;
            if scores.len() != chunk.len() {
                return Err(RankError::ModelInference {
                    model_id: self.token.descriptor().key.to_string(),
                    reason: format!(
                        "session returned {} scores for {} rows",
                        scores.len(),
                        chunk.len()
                    ),
                });
            }
            all_scores.extend(scores);
        }
        Ok(BatchOutput {
            scores: batch.docs.into_iter().zip(all_scores).collect(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{DType, ModelDescriptor, ModelFramework, ModelKey};
    use crate::model_cache::{EvictionPolicy, OnnxModelCache};
    use crate::scorer_session::{MockScorerSession, ScorerSession};
    use std::sync::Arc;

    fn descriptor(id: &str, max_batch_size: usize) -> ModelDescriptor {
        ModelDescriptor {
            key: ModelKey::new(id, "1"),
            tenant: None,
            uri: "file:///tmp/x.onnx".into(),
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

    fn install_session_returning_count(
        cache: &OnnxModelCache,
        max_batch_size: usize,
    ) -> (ScorerToken, Arc<MockScorerSession>) {
        // We need both a typed Arc to inspect the counter AND a
        // dyn-trait Arc for the cache. Trick: install the session via
        // its dyn-trait Arc; keep a separate strong ref to the same Arc
        // via downcast-friendly construction.
        let mock = Arc::new(MockScorerSession::zeros(descriptor(
            "rerank",
            max_batch_size,
        )));
        let dyn_session: Arc<dyn ScorerSession> = mock.clone();
        let token = cache.install(dyn_session);
        (token, mock)
    }

    fn batch_of(n: usize) -> BatchInput {
        let docs: Vec<DocHandle> = (0..n as u32).map(DocHandle).collect();
        let rows: Vec<Vec<f32>> = (0..n).map(|_| vec![1.0_f32]).collect();
        BatchInput::new(docs, rows)
    }

    #[test]
    fn onnx_scorer_batches_correctly_at_exact_multiple() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        let (token, mock) = install_session_returning_count(&cache, 32);
        let scorer = OnnxBatchedScorer::new(token);
        let out = scorer.score_batch(batch_of(64)).unwrap();
        assert_eq!(out.scores.len(), 64);
        // 64 / 32 = 2 calls exactly.
        assert_eq!(mock.call_count(), 2);
        assert_eq!(mock.rows_seen(), 64);
    }

    #[test]
    fn onnx_scorer_batches_correctly_with_tail() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        let (token, mock) = install_session_returning_count(&cache, 32);
        let scorer = OnnxBatchedScorer::new(token);
        let out = scorer.score_batch(batch_of(100)).unwrap();
        assert_eq!(out.scores.len(), 100);
        // ceil(100/32) = 4 calls (32 + 32 + 32 + 4).
        assert_eq!(mock.call_count(), 4);
        assert_eq!(mock.rows_seen(), 100);
    }

    #[test]
    fn onnx_scorer_handles_empty_input() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        let (token, mock) = install_session_returning_count(&cache, 32);
        let scorer = OnnxBatchedScorer::new(token);
        let out = scorer.score_batch(BatchInput::new(vec![], vec![])).unwrap();
        assert!(out.scores.is_empty());
        assert_eq!(mock.call_count(), 0, "no input → no inference calls");
    }

    #[test]
    fn onnx_scorer_rejects_mismatched_docs_and_rows() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        let (token, _) = install_session_returning_count(&cache, 32);
        let scorer = OnnxBatchedScorer::new(token);
        let bad = BatchInput::new(vec![DocHandle(0)], vec![]);
        match scorer.score_batch(bad) {
            Err(RankError::ModelInference { reason, .. }) => {
                assert!(reason.contains("docs.len()"));
            }
            other => panic!("expected ModelInference, got: {other:?}"),
        }
    }

    #[test]
    fn onnx_scorer_passes_through_docs_in_order() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        // Session that returns row[0] back as the score so we can verify ordering.
        let desc = descriptor("echo", 4);
        let echo = Arc::new(MockScorerSession::new(desc, |rows| {
            rows.iter().map(|r| r[0]).collect()
        }));
        let token = cache.install(echo);
        let scorer = OnnxBatchedScorer::new(token);
        let docs = vec![
            DocHandle(10),
            DocHandle(20),
            DocHandle(30),
            DocHandle(40),
            DocHandle(50),
        ];
        let rows = vec![vec![1.5], vec![2.5], vec![3.5], vec![4.5], vec![5.5]];
        let out = scorer
            .score_batch(BatchInput::new(docs.clone(), rows))
            .unwrap();
        for (i, (doc, score)) in out.scores.iter().enumerate() {
            assert_eq!(*doc, docs[i]);
            assert!((score - (i as f32 + 1.0 + 0.5)).abs() < 1e-5);
        }
    }

    #[test]
    fn onnx_scorer_with_batch_size_one_calls_per_doc() {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        let (token, mock) = install_session_returning_count(&cache, 1);
        let scorer = OnnxBatchedScorer::new(token);
        scorer.score_batch(batch_of(10)).unwrap();
        assert_eq!(mock.call_count(), 10);
    }

    #[test]
    fn batched_scorer_propagates_session_error_as_model_inference() {
        // A session whose closure intentionally returns the wrong row
        // count to simulate inference failure.
        struct BadSession(ModelDescriptor);
        impl ScorerSession for BadSession {
            fn descriptor(&self) -> &ModelDescriptor {
                &self.0
            }
            fn score(&self, _rows: &[Vec<f32>]) -> RankResult<Vec<f32>> {
                // Return zero scores no matter the input — exposes the
                // row-count guard in OnnxBatchedScorer.
                Ok(vec![])
            }
        }
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        let bad: Arc<dyn ScorerSession> = Arc::new(BadSession(descriptor("bad", 4)));
        let token = cache.install(bad);
        let scorer = OnnxBatchedScorer::new(token);
        match scorer.score_batch(batch_of(8)) {
            Err(RankError::ModelInference { reason, .. }) => {
                assert!(reason.contains("returned 0 scores"));
            }
            other => panic!("expected ModelInference, got: {other:?}"),
        }
    }
}
