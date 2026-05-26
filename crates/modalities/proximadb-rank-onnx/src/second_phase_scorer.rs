//! `OnnxSecondPhaseScorer` — concrete [`SecondPhaseScorer`] impl that
//! drives `OnnxBatchedScorer` against a `DocFeatureExtractor`.
//!
//! Pipeline:
//! 1. Extract per-doc input vectors via the configured extractor.
//! 2. Hand the (DocHandle, row) pairs to `OnnxBatchedScorer::score_batch`
//!    which chunks by the session's `max_batch_size` and produces one
//!    score per doc.
//! 3. Re-wrap as `ScoredHit`s with `PhaseId::SECOND`, preserving doc
//!    handles by id lookup.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` (R-7c.2).

use std::collections::HashMap;
use std::sync::Arc;

use proximadb_kernel::PhaseId;
use proximadb_rank_core::{DocHandle, QueryContext, RankResult, ScoredHit, SecondPhaseScorer};

use crate::batched_scorer::{BatchInput, BatchedScorer, OnnxBatchedScorer};
use crate::doc_feature_extractor::DocFeatureExtractor;

/// Wraps an `OnnxBatchedScorer` + a `DocFeatureExtractor` to satisfy
/// `proximadb_rank_core::SecondPhaseScorer`. The two collaborators are
/// `Arc`-held so the same scorer can be registered against multiple
/// profiles (matches the typical "one cross-encoder, many recipes"
/// deployment shape).
pub struct OnnxSecondPhaseScorer {
    inner: Arc<OnnxBatchedScorer>,
    extractor: Arc<dyn DocFeatureExtractor>,
}

impl OnnxSecondPhaseScorer {
    pub fn new(inner: Arc<OnnxBatchedScorer>, extractor: Arc<dyn DocFeatureExtractor>) -> Self {
        Self { inner, extractor }
    }
}

impl SecondPhaseScorer for OnnxSecondPhaseScorer {
    fn rescore(
        &self,
        hits: Vec<ScoredHit>,
        _qctx: &QueryContext,
    ) -> RankResult<Vec<ScoredHit>> {
        // The float-input pipeline pays no attention to qctx — the
        // pre-encoded feature rows already incorporate query state
        // upstream of this rescore. The tokenized twin
        // (`OnnxTokenizedSecondPhaseScorer`) DOES use qctx to drive
        // its tokenizer extractor.
        if hits.is_empty() {
            return Ok(Vec::new());
        }

        // Extract per-doc input rows. Errors short-circuit — better to
        // fail the second phase than emit garbage scores.
        let docs: Vec<DocHandle> = hits.iter().map(|h| h.doc).collect();
        let mut rows: Vec<Vec<f32>> = Vec::with_capacity(docs.len());
        for &doc in &docs {
            rows.push(self.extractor.extract(doc)?);
        }

        let batch = BatchInput::new(docs.clone(), rows);
        let out = self.inner.score_batch(batch)?;

        // Index by doc so the output order matches the input hit order
        // (the input hits already carry first-phase metadata we want to
        // preserve — even though we don't use it directly here).
        let score_by_doc: HashMap<DocHandle, f32> = out.scores.into_iter().collect();

        Ok(hits
            .into_iter()
            .map(|h| ScoredHit {
                doc: h.doc,
                // Missing scores fall back to 0.0 — defensive guard for
                // misbehaving sessions. The batched scorer also
                // validates row counts, so this is belt-and-suspenders.
                score: score_by_doc.get(&h.doc).copied().unwrap_or(0.0),
                phase: PhaseId::SECOND,
                // R-7c.5: preserve first-phase match_features through
                // the rescore. Rescoring changes the score but match
                // features are first-phase artifacts.
                features: h.features,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{DType, ModelDescriptor, ModelFramework, ModelKey};
    use crate::doc_feature_extractor::{FnDocFeatureExtractor, NoopDocFeatureExtractor};
    use crate::model_cache::{EvictionPolicy, OnnxModelCache};
    use crate::scorer_session::{MockScorerSession, ScorerSession};

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

    fn make_scorer(
        mock: Arc<MockScorerSession>,
        extractor: Arc<dyn DocFeatureExtractor>,
    ) -> OnnxSecondPhaseScorer {
        let cache = OnnxModelCache::new(EvictionPolicy::LruByMemory {
            budget_bytes: usize::MAX,
        });
        let dyn_session: Arc<dyn ScorerSession> = mock;
        let token = cache.install(dyn_session);
        let onnx_batched = Arc::new(OnnxBatchedScorer::new(token));
        OnnxSecondPhaseScorer::new(onnx_batched, extractor)
    }

    fn hit(doc: u32, score: f32) -> ScoredHit {
        ScoredHit::bare(DocHandle(doc), score, PhaseId::FIRST)
    }

    #[test]
    fn rescore_empty_input_short_circuits() {
        let mock = Arc::new(MockScorerSession::zeros(descriptor("m", 32)));
        let s = make_scorer(mock.clone(), Arc::new(NoopDocFeatureExtractor));
        let out = s.rescore(Vec::new(), &QueryContext::default()).unwrap();
        assert!(out.is_empty());
        assert_eq!(
            mock.call_count(),
            0,
            "no input → no extractor + no inference"
        );
    }

    #[test]
    fn rescore_tags_phase_id_second() {
        let mock = Arc::new(MockScorerSession::constant(descriptor("m", 32), 0.5));
        let s = make_scorer(mock, Arc::new(NoopDocFeatureExtractor));
        let out = s.rescore(vec![hit(1, 1.0), hit(2, 2.0)], &QueryContext::default()).unwrap();
        for h in &out {
            assert_eq!(h.phase, PhaseId::SECOND);
        }
    }

    #[test]
    fn rescore_preserves_doc_handles_in_input_order() {
        let mock = Arc::new(MockScorerSession::new(descriptor("m", 32), |rows| {
            rows.iter().map(|r| r[0]).collect()
        }));
        // Extractor returns the doc id as the row value so we can
        // verify per-doc score mapping.
        let extractor =
            Arc::new(FnDocFeatureExtractor::new(|d: DocHandle| Ok(vec![d.0 as f32])));
        let s = make_scorer(mock.clone(), extractor);

        let out = s
            .rescore(vec![hit(7, 0.9), hit(3, 0.8), hit(42, 0.7)], &QueryContext::default())
            .unwrap();
        assert_eq!(out.len(), 3);
        assert_eq!(out[0].doc, DocHandle(7));
        assert_eq!(out[0].score, 7.0);
        assert_eq!(out[1].doc, DocHandle(3));
        assert_eq!(out[1].score, 3.0);
        assert_eq!(out[2].doc, DocHandle(42));
        assert_eq!(out[2].score, 42.0);
    }

    #[test]
    fn rescore_chunks_by_max_batch_size() {
        // 100 hits with max_batch_size=32 → ceil(100/32)=4 inference calls.
        let mock = Arc::new(MockScorerSession::zeros(descriptor("m", 32)));
        let s = make_scorer(mock.clone(), Arc::new(NoopDocFeatureExtractor));
        let hits: Vec<ScoredHit> = (0..100).map(|i| hit(i, 1.0)).collect();
        let out = s.rescore(hits, &QueryContext::default()).unwrap();
        assert_eq!(out.len(), 100);
        assert_eq!(mock.call_count(), 4);
    }

    #[test]
    fn rescore_propagates_extractor_error() {
        use proximadb_rank_core::RankError;
        let mock = Arc::new(MockScorerSession::zeros(descriptor("m", 32)));
        let extractor = Arc::new(FnDocFeatureExtractor::new(|_d: DocHandle| {
            Err(RankError::ModelInference {
                model_id: "extractor".into(),
                reason: "synthetic failure".into(),
            })
        }));
        let s = make_scorer(mock.clone(), extractor);
        match s.rescore(vec![hit(1, 1.0)], &QueryContext::default()) {
            Err(RankError::ModelInference { reason, .. }) => {
                assert!(reason.contains("synthetic failure"));
            }
            Err(_) => panic!("expected ModelInference"),
            Ok(_) => panic!("expected error"),
        }
        assert_eq!(
            mock.call_count(),
            0,
            "extractor error must short-circuit before any inference call"
        );
    }
}
