//! Cross-modal fusion seam — the neutral, modality-agnostic `fuse-by-oid` core.
//!
//! This is Phase 1 (F-A) of `docs/12-design/CROSS_MODAL_FUSION_SEAM_2026_06_22.adoc`. It converges the
//! three overlapping hybrid implementations (vector+BM25 keyed by `doc_id`, vector+graph keyed by
//! `NodeId`, and the proto vector→graph path) onto ONE pipeline:
//!
//! ```text
//! seed → expand (modality expanders) → calibrate → fuse-by-oid → rank
//! ```
//!
//! correlated by the canonical `oid`. Each modality is a pluggable `ModalityExpander`; the
//! calibrate+fuse+rank core lives here and is written once.
//!
//! ## Load-bearing decision: calibration, not the operator (D3)
//!
//! Heterogeneous sources produce incommensurable score distributions — vector cosine is a narrow
//! Gaussian, graph/PPR proximity is power-law — so a naive weighted sum (and even min-max normalization)
//! is dominated by the larger-magnitude source regardless of the weights. The fix is **PIT /
//! percentile-rank** (the empirical CDF), which maps every source to ≈uniform `[0,1]` while preserving
//! within-source order. Once scores are calibrated the choice of fusion operator barely matters
//! (Calibrated-Fusion / PhaseGraph, arXiv 2603.28886), so this module keeps just two: a calibrated
//! weighted-linear blend (default) and rank-based RRF (the zero-calibration fallback). This is why the
//! graph engine's arbitrary `1/(dist+1)` transform is replaced by the percentile-rank of distance.

use std::collections::HashMap;

/// Identifies the modality a set of candidates came from. Used as the fusion `per_source` key.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum SourceId {
    Vector,
    Graph,
    Document,
    Relational,
    Other(String),
}

/// One source's per-query candidates: `oid → raw score` (higher is better) plus the source's blend
/// weight. Raw scores are calibrated inside [`Fuser::fuse`] — callers pass native scores (cosine,
/// graph proximity, BM25) without pre-normalizing.
#[derive(Debug, Clone)]
pub struct SourceCandidates {
    pub source: SourceId,
    /// Blend weight `α_k` for this source (D9: weight + selectivity aware per modality).
    pub weight: f32,
    /// `oid → raw score`, higher is better.
    pub scores: HashMap<String, f32>,
}

impl SourceCandidates {
    pub fn new(source: SourceId, weight: f32, scores: HashMap<String, f32>) -> Self {
        Self {
            source,
            weight,
            scores,
        }
    }
}

/// Cross-source score commensuration policy (applies to the [`Operator::CalibratedLinear`] path).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Calibration {
    /// PIT / percentile-rank (empirical CDF) → `[0,1]`. The default and load-bearing choice (D3).
    Pit,
    /// No calibration — use raw scores as-is. Retained only to demonstrate the failure mode and for
    /// sources that are already commensurable.
    None,
}

/// How calibrated per-source values combine into one fused score.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operator {
    /// Weighted linear over calibrated values. The default (D4).
    CalibratedLinear,
    /// Reciprocal-rank fusion — rank-based, ignores magnitudes entirely (its own calibration). The
    /// zero-calibration fallback when score distributions are unavailable/untrusted (D4).
    Rrf { k: u32 },
}

/// Fusion policy: calibration + operator + consensus boost + the cost/quality gates (D5).
#[derive(Debug, Clone)]
pub struct FusionPolicy {
    pub calibration: Calibration,
    pub operator: Operator,
    /// CombMNZ-style consensus boost added once to any `oid` present in ≥2 sources (D4/D6).
    pub consensus_beta: f32,
    /// Cap each source's candidate pool (keep top-N by raw score) before fusing — bounds the
    /// pool-explosion failure mode (D5).
    pub pool_cap: Option<usize>,
    /// Skip a source whose best raw score is below this threshold (saturation / didn't-fire gate, D5).
    pub min_source_score: Option<f32>,
}

impl Default for FusionPolicy {
    fn default() -> Self {
        Self {
            calibration: Calibration::Pit,
            operator: Operator::CalibratedLinear,
            consensus_beta: 0.0,
            pool_cap: None,
            min_source_score: None,
        }
    }
}

impl FusionPolicy {
    /// The zero-calibration rank-fusion fallback (standard `k = 60`).
    pub fn rrf() -> Self {
        Self {
            calibration: Calibration::None,
            operator: Operator::Rrf { k: 60 },
            ..Self::default()
        }
    }
}

/// One fused result, keyed by the canonical `oid` (dedup + co-score across sources).
#[derive(Debug, Clone)]
pub struct FusedItem {
    pub oid: String,
    /// Combined fused score (higher is better).
    pub score: f32,
    /// Calibrated contribution per source (for explainability / debugging).
    pub per_source: HashMap<SourceId, f32>,
    /// Number of sources that contained this `oid`.
    pub source_count: usize,
}

/// Fusion bookkeeping for route metadata / observability.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FusionStats {
    pub sources_fused: usize,
    pub sources_skipped: usize,
    pub candidates_in: usize,
    pub items_out: usize,
}

/// The neutral calibrate + fuse + rank core. Modality-agnostic: it operates only on `(oid, score)`
/// lists, so it is unit-testable in isolation and reused by every modality.
pub struct Fuser {
    policy: FusionPolicy,
}

impl Fuser {
    pub fn new(policy: FusionPolicy) -> Self {
        Self { policy }
    }

    /// Fuse per-source candidates by `oid`, returning the top-`limit` items (descending score) plus
    /// stats. Empty / below-threshold sources are skipped; surviving sources are pool-capped,
    /// calibrated, weighted, merged by `oid`, and consensus-boosted.
    pub fn fuse(
        &self,
        sources: Vec<SourceCandidates>,
        limit: usize,
    ) -> (Vec<FusedItem>, FusionStats) {
        let mut stats = FusionStats::default();
        let mut acc: HashMap<String, FusedItem> = HashMap::new();

        for mut src in sources {
            stats.candidates_in += src.scores.len();
            let best = src
                .scores
                .values()
                .copied()
                .fold(f32::NEG_INFINITY, f32::max);
            // Skip-gate (D5): an empty source, or one whose best score is below the usefulness
            // threshold (it didn't meaningfully fire), is dropped rather than diluting the blend.
            if src.scores.is_empty() || self.policy.min_source_score.is_some_and(|min| best < min) {
                stats.sources_skipped += 1;
                continue;
            }
            // Pool cap (D5): bound each source to its top-N by raw score before fusing.
            if let Some(cap) = self.policy.pool_cap
                && src.scores.len() > cap
            {
                let mut entries: Vec<(String, f32)> = src.scores.into_iter().collect();
                entries.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
                entries.truncate(cap);
                src.scores = entries.into_iter().collect();
            }
            stats.sources_fused += 1;

            for (oid, value) in self.calibrate(&src) {
                let contribution = src.weight * value;
                let item = acc.entry(oid.clone()).or_insert_with(|| FusedItem {
                    oid,
                    score: 0.0,
                    per_source: HashMap::new(),
                    source_count: 0,
                });
                item.score += contribution;
                item.per_source.insert(src.source.clone(), value);
                item.source_count += 1;
            }
        }

        // Consensus boost (D4/D6): reward an `oid` that ≥2 sources agree on.
        if self.policy.consensus_beta != 0.0 {
            for item in acc.values_mut() {
                if item.source_count >= 2 {
                    item.score += self.policy.consensus_beta;
                }
            }
        }

        let mut items: Vec<FusedItem> = acc.into_values().collect();
        // Deterministic order: score desc, then oid asc to break ties.
        items.sort_by(|a, b| b.score.total_cmp(&a.score).then_with(|| a.oid.cmp(&b.oid)));
        items.truncate(limit);
        stats.items_out = items.len();
        (items, stats)
    }

    /// Map a source's raw `oid → score` to calibrated `oid → value` per the policy.
    fn calibrate(&self, src: &SourceCandidates) -> Vec<(String, f32)> {
        match self.policy.operator {
            // RRF is its own calibration: value = 1/(k + rank), rank by raw score (best = 1).
            Operator::Rrf { k } => {
                let mut ranked: Vec<(&String, f32)> =
                    src.scores.iter().map(|(oid, s)| (oid, *s)).collect();
                ranked.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(b.0)));
                ranked
                    .into_iter()
                    .enumerate()
                    .map(|(i, (oid, _))| {
                        let rank = (i + 1) as f32;
                        (oid.clone(), 1.0 / (k as f32 + rank))
                    })
                    .collect()
            }
            Operator::CalibratedLinear => match self.policy.calibration {
                Calibration::None => src
                    .scores
                    .iter()
                    .map(|(oid, s)| (oid.clone(), *s))
                    .collect(),
                // PIT / percentile-rank: value(oid) = |{ scores ≤ s }| / N — the empirical CDF.
                Calibration::Pit => {
                    let mut sorted: Vec<f32> = src.scores.values().copied().collect();
                    sorted.sort_by(f32::total_cmp);
                    let n = sorted.len() as f32;
                    src.scores
                        .iter()
                        .map(|(oid, s)| {
                            let le = sorted
                                .partition_point(|x| x.total_cmp(s) != std::cmp::Ordering::Greater);
                            (oid.clone(), le as f32 / n)
                        })
                        .collect()
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn src(source: SourceId, weight: f32, pairs: &[(&str, f32)]) -> SourceCandidates {
        SourceCandidates::new(
            source,
            weight,
            pairs.iter().map(|(o, s)| ((*o).to_string(), *s)).collect(),
        )
    }

    fn score_of(items: &[FusedItem], oid: &str) -> Option<f32> {
        items.iter().find(|i| i.oid == oid).map(|i| i.score)
    }

    /// The headline property (D3): uncalibrated weighted-sum is dominated by the larger-magnitude
    /// source (graph proximity here), so it picks the graph-favored oid decisively; PIT/percentile-rank
    /// makes the two sources commensurable so the rank-balanced oids tie. Same inputs, different policy.
    #[test]
    fn pit_calibration_makes_heterogeneous_scores_commensurable() {
        // Vector: narrow Gaussian-ish magnitudes; Graph: a power-law spike (x huge, y tiny).
        let vector = src(SourceId::Vector, 1.0, &[("x", 0.10), ("y", 0.12)]);
        let graph = src(SourceId::Graph, 1.0, &[("x", 0.30), ("y", 0.003)]);

        // Uncalibrated: x = 0.10 + 0.30 = 0.40, y = 0.12 + 0.003 = 0.123 → x wins by the graph magnitude.
        let none = Fuser::new(FusionPolicy {
            calibration: Calibration::None,
            ..FusionPolicy::default()
        });
        let (raw, _) = none.fuse(vec![vector.clone(), graph.clone()], 10);
        assert_eq!(raw[0].oid, "x");
        assert!(
            raw[0].score - raw[1].score > 0.2,
            "uncalibrated: graph magnitude dominates the blend"
        );

        // PIT: x = pct(0.5 vec)+pct(1.0 graph) = 1.5 ; y = pct(1.0)+pct(0.5) = 1.5 → tie.
        let pit = Fuser::new(FusionPolicy::default());
        let (cal, stats) = pit.fuse(vec![vector, graph], 10);
        assert_eq!(stats.sources_fused, 2);
        assert!(
            (score_of(&cal, "x").unwrap() - score_of(&cal, "y").unwrap()).abs() < 1e-6,
            "PIT: rank-balanced oids are commensurable (tie)"
        );
    }

    /// Fusion merges by canonical `oid`: an `oid` in two sources appears once, co-scored, with
    /// `source_count == 2`.
    #[test]
    fn fuse_by_oid_dedups_and_counts_sources() {
        let a = src(SourceId::Vector, 1.0, &[("z", 0.5), ("a", 0.1)]);
        let b = src(SourceId::Graph, 1.0, &[("z", 0.5), ("b", 0.2)]);
        let (items, _) = Fuser::new(FusionPolicy::default()).fuse(vec![a, b], 10);

        assert_eq!(
            items.iter().filter(|i| i.oid == "z").count(),
            1,
            "z deduped"
        );
        let z = items.iter().find(|i| i.oid == "z").unwrap();
        assert_eq!(z.source_count, 2);
        assert_eq!(z.per_source.len(), 2);
        for single in items.iter().filter(|i| i.oid != "z") {
            assert_eq!(single.source_count, 1);
        }
    }

    /// Consensus boost (D4/D6) adds exactly `beta` to a multi-source `oid` and nothing to a
    /// single-source one — co-agreement is rewarded.
    #[test]
    fn consensus_boost_rewards_multi_source_agreement() {
        let a = src(SourceId::Vector, 1.0, &[("p", 0.9), ("q", 0.95)]);
        let b = src(SourceId::Graph, 1.0, &[("p", 0.9)]); // p in both, q in one
        let inputs = || vec![a.clone(), b.clone()];

        let base = Fuser::new(FusionPolicy::default()).fuse(inputs(), 10).0;
        let boosted = Fuser::new(FusionPolicy {
            consensus_beta: 0.5,
            ..FusionPolicy::default()
        })
        .fuse(inputs(), 10)
        .0;

        assert!(
            (score_of(&boosted, "p").unwrap() - score_of(&base, "p").unwrap() - 0.5).abs() < 1e-6,
            "p (2 sources) gains exactly beta"
        );
        assert!(
            (score_of(&boosted, "q").unwrap() - score_of(&base, "q").unwrap()).abs() < 1e-6,
            "q (1 source) is unchanged"
        );
    }

    /// Skip-gate (D5): a source whose best score is below `min_source_score`, or an empty source, is
    /// dropped (counted in stats) and its candidates never reach the blend.
    #[test]
    fn skip_gate_drops_weak_and_empty_sources() {
        let strong = src(SourceId::Vector, 1.0, &[("a", 0.9), ("b", 0.8)]);
        let weak = src(SourceId::Graph, 1.0, &[("c", 0.01)]); // best 0.01 < 0.1
        let empty = src(SourceId::Document, 1.0, &[]);
        let policy = FusionPolicy {
            min_source_score: Some(0.1),
            ..FusionPolicy::default()
        };
        let (items, stats) = Fuser::new(policy).fuse(vec![strong, weak, empty], 10);

        assert_eq!(stats.sources_fused, 1);
        assert_eq!(stats.sources_skipped, 2);
        assert!(!items.iter().any(|i| i.oid == "c"), "weak source dropped");
    }

    /// RRF fallback (D4) ranks by reciprocal rank without any magnitude — produces all candidates and a
    /// sensible order even with wildly different score scales.
    #[test]
    fn rrf_fallback_ranks_without_magnitudes() {
        let a = src(SourceId::Vector, 1.0, &[("a", 0.1), ("b", 0.2), ("c", 0.3)]);
        let b = src(SourceId::Graph, 1.0, &[("a", 900.0), ("c", 0.1)]); // huge magnitude must NOT dominate
        let (items, _) = Fuser::new(FusionPolicy::rrf()).fuse(vec![a, b], 10);

        assert_eq!(items.len(), 3);
        // a (rank3 in A + rank1 in B) and c (rank1 in A + rank2 in B) are the consensus top-2; b (one
        // source, rank2) is last — magnitude (900) did not dominate.
        assert_eq!(items[2].oid, "b");
        let top2: Vec<&str> = items[..2].iter().map(|i| i.oid.as_str()).collect();
        assert!(top2.contains(&"a") && top2.contains(&"c"));
    }

    /// Pool cap (D5) bounds each source to its top-N by raw score before fusing.
    #[test]
    fn pool_cap_bounds_each_source() {
        let big = src(
            SourceId::Vector,
            1.0,
            &[("a", 0.1), ("b", 0.2), ("c", 0.3), ("d", 0.4)],
        );
        let policy = FusionPolicy {
            pool_cap: Some(2),
            ..FusionPolicy::default()
        };
        let (items, _) = Fuser::new(policy).fuse(vec![big], 10);
        // Only the top-2 (c, d) survive the cap.
        assert_eq!(items.len(), 2);
        let oids: Vec<&str> = items.iter().map(|i| i.oid.as_str()).collect();
        assert!(oids.contains(&"c") && oids.contains(&"d"));
    }
}
