//! `OnnxTokenizedSecondPhaseScorer` — concrete `SecondPhaseScorer`
//! impl that drives `OnnxTokenizedBatchedScorer` against a
//! [`TokenizedDocFeatureExtractor`]. R-5b.1.1.
//!
//! Twin of [`OnnxSecondPhaseScorer`](crate::OnnxSecondPhaseScorer);
//! same role in the pipeline (rescore the first-phase top-K) but
//! drives the tokenized branch: BERT-family cross-encoders that
//! take int64 token tensors instead of pre-encoded floats.
//!
//! Pipeline:
//! 1. Pull DocHandles from the input hits.
//! 2. Build a `TokenizedBatch` for the full set via the extractor
//!    (one `tokenizers::encode_batch` call in production deployments).
//! 3. Hand off to `OnnxTokenizedBatchedScorer::score_batch`, which
//!    chunks by `descriptor.max_batch_size` and invokes the session.
//! 4. Re-wrap as `ScoredHit`s with `PhaseId::SECOND`, preserving
//!    per-doc `match_features` captured during first phase (R-7c.5).
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` §4.4.

use std::collections::HashMap;
use std::sync::Arc;

use proximadb_kernel::PhaseId;
use proximadb_rank_core::{DocHandle, RankResult, ScoredHit, SecondPhaseScorer};

use crate::tokenized_batched_scorer::{
    OnnxTokenizedBatchedScorer, TokenizedBatchInput, TokenizedBatchedScorer,
};
use crate::tokenized_doc_feature_extractor::TokenizedDocFeatureExtractor;

/// Wraps an `OnnxTokenizedBatchedScorer` + a `TokenizedDocFeatureExtractor`
/// to satisfy `proximadb_rank_core::SecondPhaseScorer`. Both
/// collaborators are `Arc`-held — the same scorer + extractor pair
/// can be registered against multiple profiles (the typical "one BERT
/// cross-encoder, many recipes" deployment shape).
pub struct OnnxTokenizedSecondPhaseScorer {
    inner: Arc<OnnxTokenizedBatchedScorer>,
    extractor: Arc<dyn TokenizedDocFeatureExtractor>,
}

impl OnnxTokenizedSecondPhaseScorer {
    pub fn new(
        inner: Arc<OnnxTokenizedBatchedScorer>,
        extractor: Arc<dyn TokenizedDocFeatureExtractor>,
    ) -> Self {
        Self { inner, extractor }
    }
}

impl SecondPhaseScorer for OnnxTokenizedSecondPhaseScorer {
    fn rescore(&self, hits: Vec<ScoredHit>) -> RankResult<Vec<ScoredHit>> {
        if hits.is_empty() {
            return Ok(Vec::new());
        }
        let docs: Vec<DocHandle> = hits.iter().map(|h| h.doc).collect();
        let batch = self.extractor.extract_batch(&docs)?;
        let input = TokenizedBatchInput::new(docs.clone(), batch);
        let out = self.inner.score_batch(input)?;

        // Order-preserving re-wrap. Same fallback policy as
        // OnnxSecondPhaseScorer: missing scores default to 0.0 (the
        // batched scorer already validates row counts, so this only
        // fires on a buggy session that the row-count guard misses).
        let score_by_doc: HashMap<DocHandle, f32> = out.scores.into_iter().collect();
        Ok(hits
            .into_iter()
            .map(|h| ScoredHit {
                doc: h.doc,
                score: score_by_doc.get(&h.doc).copied().unwrap_or(0.0),
                phase: PhaseId::SECOND,
                // R-7c.5: preserve first-phase match_features through
                // the rescore — same contract as the float-input twin.
                features: h.features,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::descriptor::{DType, ModelDescriptor, ModelFramework, ModelKey};
    use crate::tokenized_doc_feature_extractor::{
        FnTokenizedDocFeatureExtractor, NoopTokenizedDocFeatureExtractor,
    };
    use crate::tokenized_scorer_session::{
        MockTokenizedScorerSession, TokenizedBatch, TokenizedScorerSession,
    };

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
        session: Arc<dyn TokenizedScorerSession>,
        extractor: Arc<dyn TokenizedDocFeatureExtractor>,
    ) -> OnnxTokenizedSecondPhaseScorer {
        let inner = Arc::new(OnnxTokenizedBatchedScorer::new(session));
        OnnxTokenizedSecondPhaseScorer::new(inner, extractor)
    }

    fn hit(doc: u32, score: f32) -> ScoredHit {
        ScoredHit::bare(DocHandle(doc), score, PhaseId::FIRST)
    }

    // ---------------- Pipeline integration ----------------

    #[test]
    fn empty_hits_short_circuits_with_no_extractor_or_session_call() {
        let session: Arc<dyn TokenizedScorerSession> =
            Arc::new(MockTokenizedScorerSession::zeros(descriptor("r", 32)));
        let extractor: Arc<dyn TokenizedDocFeatureExtractor> =
            Arc::new(NoopTokenizedDocFeatureExtractor::default());
        let scorer = make_scorer(session, extractor);
        let out = scorer.rescore(Vec::new()).unwrap();
        assert!(out.is_empty());
    }

    #[test]
    fn rescore_runs_extractor_then_session_and_tags_phase_id_second() {
        let session: Arc<dyn TokenizedScorerSession> =
            Arc::new(MockTokenizedScorerSession::constant(descriptor("r", 32), 0.75));
        let extractor: Arc<dyn TokenizedDocFeatureExtractor> =
            Arc::new(NoopTokenizedDocFeatureExtractor::default());
        let scorer = make_scorer(session, extractor);

        let inputs = vec![hit(1, 1.0), hit(2, 2.0), hit(3, 3.0)];
        let out = scorer.rescore(inputs).unwrap();
        assert_eq!(out.len(), 3);
        for h in &out {
            assert_eq!(h.phase, PhaseId::SECOND);
            assert!((h.score - 0.75).abs() < 1e-5);
        }
    }

    #[test]
    fn rescore_preserves_doc_order_from_input_hits() {
        // The input hit order should match the output hit order even
        // when chunk boundaries shuffle the underlying inference calls.
        let session: Arc<dyn TokenizedScorerSession> =
            Arc::new(MockTokenizedScorerSession::new(descriptor("echo", 2), |b| {
                // Score = first token id (already validated to be one
                // i64 per row by validate_rectangular).
                b.input_ids.iter().map(|r| r[0] as f32).collect()
            }));
        let extractor: Arc<dyn TokenizedDocFeatureExtractor> =
            Arc::new(FnTokenizedDocFeatureExtractor::new(|docs| {
                Ok(TokenizedBatch::new(
                    docs.iter().map(|d| vec![d.0 as i64]).collect(),
                    docs.iter().map(|_| vec![1i64]).collect(),
                ))
            }));
        let scorer = make_scorer(session, extractor);

        let inputs = vec![hit(10, 0.0), hit(20, 0.0), hit(30, 0.0), hit(40, 0.0)];
        let out = scorer.rescore(inputs).unwrap();
        assert_eq!(out.len(), 4);
        assert_eq!(out[0].doc, DocHandle(10));
        assert_eq!(out[1].doc, DocHandle(20));
        assert_eq!(out[2].doc, DocHandle(30));
        assert_eq!(out[3].doc, DocHandle(40));
        assert!((out[0].score - 10.0).abs() < 1e-5);
        assert!((out[1].score - 20.0).abs() < 1e-5);
        assert!((out[2].score - 30.0).abs() < 1e-5);
        assert!((out[3].score - 40.0).abs() < 1e-5);
    }

    #[test]
    fn rescore_preserves_first_phase_match_features_through_rescore() {
        // R-7c.5 contract: rescoring changes score + phase, NOT
        // match_features. The tokenized variant must honor the same
        // invariant the float-input twin does.
        use proximadb_rank_core::ScoredHit as Hit;
        let features: Arc<[(Arc<str>, f32)]> =
            Arc::from(vec![(Arc::<str>::from("bm25(title)"), 11.5_f32)]);

        let session: Arc<dyn TokenizedScorerSession> =
            Arc::new(MockTokenizedScorerSession::constant(descriptor("r", 32), 0.5));
        let extractor: Arc<dyn TokenizedDocFeatureExtractor> =
            Arc::new(NoopTokenizedDocFeatureExtractor::default());
        let scorer = make_scorer(session, extractor);

        let inputs = vec![Hit {
            doc: DocHandle(1),
            score: 1.0,
            phase: PhaseId::FIRST,
            features: Some(features.clone()),
        }];
        let out = scorer.rescore(inputs).unwrap();
        assert_eq!(out[0].phase, PhaseId::SECOND);
        assert!((out[0].score - 0.5).abs() < 1e-5);
        assert!(Arc::ptr_eq(out[0].features.as_ref().unwrap(), &features));
    }

    #[test]
    fn rescore_propagates_extractor_error() {
        // An extractor that errors should short-circuit before any
        // inference work happens (defensive: malformed doc text /
        // missing attributes is preferable to silently scoring bogus
        // tokens).
        use proximadb_rank_core::RankError;
        let mock_session = Arc::new(MockTokenizedScorerSession::zeros(descriptor("r", 32)));
        let session: Arc<dyn TokenizedScorerSession> = mock_session.clone();
        let extractor: Arc<dyn TokenizedDocFeatureExtractor> =
            Arc::new(FnTokenizedDocFeatureExtractor::new(|_| {
                Err(RankError::ModelInference {
                    model_id: "extractor".into(),
                    reason: "doc text missing".into(),
                })
            }));
        let scorer = make_scorer(session, extractor);

        let inputs = vec![hit(1, 1.0)];
        match scorer.rescore(inputs) {
            Err(RankError::ModelInference { reason, .. }) => {
                assert!(reason.contains("doc text missing"));
            }
            other => panic!("expected ModelInference, got {other:?}"),
        }
        // Extractor failure → session was never called.
        assert_eq!(mock_session.call_count(), 0);
    }

    #[test]
    fn rescore_chunks_by_max_batch_size_through_full_pipeline() {
        // End-to-end check that the second-phase scorer respects
        // descriptor.max_batch_size by issuing multiple session calls
        // for input larger than one batch.
        let mock_session = Arc::new(MockTokenizedScorerSession::zeros(descriptor("r", 4)));
        let session: Arc<dyn TokenizedScorerSession> = mock_session.clone();
        let extractor: Arc<dyn TokenizedDocFeatureExtractor> =
            Arc::new(NoopTokenizedDocFeatureExtractor::default());
        let scorer = make_scorer(session, extractor);

        let inputs: Vec<ScoredHit> = (1..=10).map(|i| hit(i, 0.0)).collect();
        let out = scorer.rescore(inputs).unwrap();
        assert_eq!(out.len(), 10);
        assert_eq!(mock_session.call_count(), 3); // 4 + 4 + 2
        assert_eq!(mock_session.rows_seen(), 10);
    }
}
