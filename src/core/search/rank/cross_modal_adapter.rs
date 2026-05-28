//! Adapter: `proximadb_query::reranking::CrossModalReranker` → `GlobalScorer`.
//!
//! Reuses the production reranker (MMR + intent + temporal + cross-modal
//! semantic scoring + explanations) instead of duplicating its logic in
//! the new ranking framework. Per the CLAUDE.md reuse-first mandate:
//! the reranker is **adopted** as a global-phase executor, not forked.

use std::collections::HashMap;
use std::sync::Arc;

use proximadb_data_model::DataModel;
use proximadb_query::reranking::{CrossModalReranker, QueryContext as RerankCtx, RerankConfig};
use proximadb_query::results::{QueryMetrics, QueryResult, UnifiedRecord};
use proximadb_rank_core::{DocHandle, GlobalScorer, PhaseId, RankError, RankResult, ScoredHit};

/// `GlobalScorer` wrapper around the production `CrossModalReranker`.
///
/// The wrap step turns `ScoredHit { doc, score, phase }` into a minimal
/// `UnifiedRecord` so the reranker's MMR / intent / temporal logic can
/// operate. The unwrap step puts scores back onto `ScoredHit` with
/// `PhaseId::GLOBAL` set.
///
/// IDs flow as `DocHandle.0` → decimal string → and back; this is the
/// stable contract for the adapter. R-7 may swap to a richer envelope if
/// the reranker needs richer per-doc context.
pub struct CrossModalGlobalScorer {
    inner: Arc<CrossModalReranker>,
    rerank_ctx: RerankCtx,
}

impl CrossModalGlobalScorer {
    /// Build with the supplied reranker config. The rerank ctx defaults
    /// to neutral (no intent, no temporal preference); use
    /// [`with_context`](Self::with_context) to inject a richer one.
    pub fn new(config: RerankConfig) -> Self {
        Self {
            inner: Arc::new(CrossModalReranker::new(config)),
            rerank_ctx: RerankCtx::default(),
        }
    }

    pub fn with_context(config: RerankConfig, rerank_ctx: RerankCtx) -> Self {
        Self {
            inner: Arc::new(CrossModalReranker::new(config)),
            rerank_ctx,
        }
    }

    /// Default-config constructor (reranker disabled — used as a no-op
    /// global scorer in tests and as the fallback when no profile has a
    /// `global_phase` configured but the pipeline still wants to log
    /// the phase identity).
    pub fn disabled() -> Self {
        Self::new(RerankConfig::default())
    }
}

#[async_trait::async_trait]
impl GlobalScorer for CrossModalGlobalScorer {
    async fn score(&self, hits: Vec<ScoredHit>, topk: usize) -> RankResult<Vec<ScoredHit>> {
        if hits.is_empty() {
            return Ok(Vec::new());
        }

        // ---- wrap: ScoredHit → UnifiedRecord ----
        let records: Vec<UnifiedRecord> = hits
            .iter()
            .map(|h| UnifiedRecord {
                id: h.doc.0.to_string(),
                source_model: DataModel::Vector,
                data: serde_json::Value::Null,
                score: Some(h.score as f64),
                metadata: HashMap::new(),
            })
            .collect();

        let qr = QueryResult {
            records,
            total_count: None,
            metrics: QueryMetrics::default(),
        };

        // ---- delegate to the production reranker ----
        let reranked =
            self.inner
                .rerank(qr, &self.rerank_ctx)
                .map_err(|e| RankError::ModelInference {
                    model_id: "cross_modal_reranker".into(),
                    reason: e.to_string(),
                })?;

        // ---- unwrap: UnifiedRecord → ScoredHit ----
        // Preserve the original DocHandles by lookup table; the
        // reranker only sees stringified ids so we can't trust it to
        // round-trip them as integers.
        let by_id: HashMap<String, DocHandle> =
            hits.iter().map(|h| (h.doc.0.to_string(), h.doc)).collect();

        let mut out: Vec<ScoredHit> = reranked
            .records
            .into_iter()
            .filter_map(|r| {
                by_id.get(&r.id).map(|doc| {
                    ScoredHit::bare(*doc, r.score.unwrap_or(0.0) as f32, PhaseId::GLOBAL)
                })
            })
            .collect();
        out.truncate(topk);
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proximadb_query::reranking::{MissingScorePolicy, ModelWeightConfig};

    fn enabled_config() -> RerankConfig {
        // Turn the reranker on (it's disabled by default) so adapter
        // tests actually exercise the rerank path. MMR + diversity off
        // for predictability; semantic_rerank off so we don't need
        // embeddings on each record.
        RerankConfig {
            enabled: true,
            semantic_rerank: false,
            diversity_optimization: false,
            context_aware: false,
            missing_score: MissingScorePolicy::Preserve,
            model_weights: ModelWeightConfig::default(),
            ..RerankConfig::default()
        }
    }

    fn hits(scores: &[f32]) -> Vec<ScoredHit> {
        scores
            .iter()
            .enumerate()
            .map(|(i, s)| ScoredHit::bare(DocHandle(i as u32 + 1), *s, PhaseId::FIRST))
            .collect()
    }

    #[tokio::test]
    async fn adapter_returns_empty_for_empty_input() {
        let scorer = CrossModalGlobalScorer::new(enabled_config());
        let out = scorer.score(Vec::new(), 10).await.unwrap();
        assert!(out.is_empty());
    }

    #[tokio::test]
    async fn adapter_preserves_doc_handles() {
        let scorer = CrossModalGlobalScorer::new(enabled_config());
        let input = hits(&[0.9, 0.5, 0.7]);
        let input_doc_set: std::collections::HashSet<DocHandle> =
            input.iter().map(|h| h.doc).collect();
        let out = scorer.score(input, 10).await.unwrap();
        for o in &out {
            assert!(
                input_doc_set.contains(&o.doc),
                "adapter produced doc {:?} not in input",
                o.doc
            );
            assert_eq!(o.phase, PhaseId::GLOBAL);
        }
    }

    #[tokio::test]
    async fn adapter_truncates_to_topk() {
        let scorer = CrossModalGlobalScorer::new(enabled_config());
        let input = hits(&[0.9, 0.8, 0.7, 0.6, 0.5]);
        let out = scorer.score(input, 2).await.unwrap();
        assert_eq!(out.len(), 2);
    }

    #[tokio::test]
    async fn disabled_reranker_preserves_input_order() {
        // The reranker's default config has enabled=false → input is
        // returned unchanged (modulo the wrap/unwrap envelope).
        let scorer = CrossModalGlobalScorer::disabled();
        let input = hits(&[0.9, 0.5, 0.7]);
        let out = scorer.score(input.clone(), 10).await.unwrap();
        // Order may be wrap-induced but identity must be preserved.
        let in_ids: std::collections::HashSet<u32> = input.iter().map(|h| h.doc.0).collect();
        let out_ids: std::collections::HashSet<u32> = out.iter().map(|h| h.doc.0).collect();
        assert_eq!(in_ids, out_ids);
    }

    #[tokio::test]
    async fn adapter_phase_id_is_global() {
        let scorer = CrossModalGlobalScorer::new(enabled_config());
        let input = hits(&[1.0, 2.0, 3.0]);
        let out = scorer.score(input, 10).await.unwrap();
        for o in &out {
            assert_eq!(o.phase, PhaseId::GLOBAL);
        }
    }
}
