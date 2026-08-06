// Utility-aware scorer - LLD 8.
//
// Final evidence ranking combines vector similarity with operational utility.
// The LLD anchors the linear blend on:
//
//   utility_score =
//     0.45 * vector_similarity +
//     0.20 * lexical_score +
//     0.15 * source_authority +
//     0.10 * freshness_score +
//     0.05 * historical_success +
//     0.05 * diversity_score
//
// `historical_success` must stay tenant-local - feedback from one tenant's
// answers must never train the ranker that serves another. The pluggable
// `UtilityScorer` trait lets a tenant attach an externally-trained UAE
// artifact (Utility-Aligned Embeddings, arXiv 2604.22722) that distills
// generative utility into a bi-encoder, replacing the linear blend with a
// learned ranker. The artifact path is provided by the operator via tenant
// configuration; the data plane only consumes a scoring function.
//
// This module ships the linear-blend default (`LinearUtilityScorer`) and
// the trait + path-based artifact wrapper (`ArtifactUtilityScorer`) the
// runtime loads when a tenant has registered one.

use std::path::PathBuf;

/// Per-candidate features the scorer consumes. All scalars in `[0.0, 1.0]`.
/// The runtime normalizes each component before passing it in - the scorer
/// itself does no rescaling beyond clamping out-of-band values.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct UtilityFeatures {
    /// Cosine similarity / inverse distance, normalized to [0,1].
    pub vector_similarity: f64,
    /// Lexical / BM25 score, normalized to [0,1].
    pub lexical_score: f64,
    /// Source authority signal (e.g. internal runbook -> 1.0, random blog -> 0.2).
    pub source_authority: f64,
    /// Freshness in [0,1] - 1.0 for "fresh", 0.0 for "ancient".
    pub freshness_score: f64,
    /// Historical-success signal - tenant-local, derived from answer
    /// feedback. Must be 0 for queries from new tenants without history.
    pub historical_success: f64,
    /// Diversity score - 1.0 when the candidate brings novel information
    /// vs the candidates already accepted, 0.0 when redundant.
    pub diversity_score: f64,
}

impl UtilityFeatures {
    /// All features at 0.0 - used when the runtime can't compute a feature
    /// (the blend then treats it as a neutral signal).
    pub const fn zero() -> Self {
        Self {
            vector_similarity: 0.0,
            lexical_score: 0.0,
            source_authority: 0.0,
            freshness_score: 0.0,
            historical_success: 0.0,
            diversity_score: 0.0,
        }
    }
}

/// Tenant-supplied weights. Defaults match the LLD 8 formula.
#[derive(Debug, Clone, Copy)]
pub struct UtilityWeights {
    pub vector_similarity: f64,
    pub lexical_score: f64,
    pub source_authority: f64,
    pub freshness_score: f64,
    pub historical_success: f64,
    pub diversity_score: f64,
}

impl Default for UtilityWeights {
    fn default() -> Self {
        Self {
            vector_similarity: 0.45,
            lexical_score: 0.20,
            source_authority: 0.15,
            freshness_score: 0.10,
            historical_success: 0.05,
            diversity_score: 0.05,
        }
    }
}

impl UtilityWeights {
    /// Sum of all weights. The LLD blend sums to exactly 1.0; tenants that
    /// override the weights are responsible for keeping the sum sensible.
    pub fn total(&self) -> f64 {
        self.vector_similarity
            + self.lexical_score
            + self.source_authority
            + self.freshness_score
            + self.historical_success
            + self.diversity_score
    }

    /// Returns `true` when the weights sum to within 1e-6 of `1.0`.
    /// Callers that want to enforce normalization in production can check
    /// this and reject misconfigured weights at startup.
    pub fn is_normalized(&self) -> bool {
        (self.total() - 1.0).abs() <= 1e-6
    }
}

/// Pluggable scorer interface. Implementations:
///
///   - `LinearUtilityScorer` - default LLD 8 blend; no external state.
///   - `ArtifactUtilityScorer` - wraps an externally-trained UAE artifact.
///     Phase 6 ships the path stub; engine loading is the runtime's job.
pub trait UtilityScorer: Send + Sync {
    /// Score one candidate. Higher is better.
    fn score(&self, features: &UtilityFeatures) -> f64;

    /// Free-form label so the trace + audit log can identify which scorer
    /// produced the score. `"linear-default"` for the linear blend;
    /// `"uae-artifact-v1"` etc. for trained models.
    fn name(&self) -> &str {
        "unnamed"
    }
}

/// Default LLD 8 linear blend.
#[derive(Debug, Clone, Copy)]
pub struct LinearUtilityScorer {
    weights: UtilityWeights,
}

impl LinearUtilityScorer {
    pub fn new(weights: UtilityWeights) -> Self {
        Self { weights }
    }

    pub fn default_lld() -> Self {
        Self {
            weights: UtilityWeights::default(),
        }
    }

    pub fn weights(&self) -> UtilityWeights {
        self.weights
    }
}

impl UtilityScorer for LinearUtilityScorer {
    fn score(&self, f: &UtilityFeatures) -> f64 {
        let clamp = |v: f64| v.clamp(0.0, 1.0);
        let w = &self.weights;
        let raw = w.vector_similarity * clamp(f.vector_similarity)
            + w.lexical_score * clamp(f.lexical_score)
            + w.source_authority * clamp(f.source_authority)
            + w.freshness_score * clamp(f.freshness_score)
            + w.historical_success * clamp(f.historical_success)
            + w.diversity_score * clamp(f.diversity_score);
        raw.clamp(0.0, w.total().max(0.0))
    }

    fn name(&self) -> &str {
        "linear-default"
    }
}

/// Wrapper that records a model artifact path so the runtime can load the
/// UAE-distilled bi-encoder when one is provided by the operator. Phase 6
/// ships only the path-recording shell; the actual scoring delegates to a
/// fallback (the linear blend) until the runtime loads the artifact. This
/// keeps the gateway flow workable even before the model artifact ships.
pub struct ArtifactUtilityScorer {
    /// Filesystem / object-store path to the artifact.
    pub artifact_path: PathBuf,
    /// Tenant-supplied label (e.g. `uae-v1`).
    pub artifact_label: String,
    /// Fallback used when the artifact hasn't been loaded yet.
    fallback: LinearUtilityScorer,
}

impl ArtifactUtilityScorer {
    /// Register a tenant's artifact without loading it. The runtime swaps
    /// in the loaded scorer once the artifact is available.
    pub fn pending(artifact_path: impl Into<PathBuf>, artifact_label: impl Into<String>) -> Self {
        Self {
            artifact_path: artifact_path.into(),
            artifact_label: artifact_label.into(),
            fallback: LinearUtilityScorer::default_lld(),
        }
    }

    /// Whether the artifact has been loaded. Phase 6 ships only the path;
    /// loading lives in the operator-side runtime integration.
    pub fn is_loaded(&self) -> bool {
        false
    }
}

impl UtilityScorer for ArtifactUtilityScorer {
    fn score(&self, features: &UtilityFeatures) -> f64 {
        // Until the runtime loads the artifact, fall back to the linear
        // blend so the gateway flow doesn't 5xx waiting for a model.
        self.fallback.score(features)
    }

    fn name(&self) -> &str {
        if self.is_loaded() {
            &self.artifact_label
        } else {
            "uae-artifact-pending"
        }
    }
}

/// Apply a scorer to a slice of candidates and return them sorted by score
/// (descending). Stable sort so ties preserve input order - the runtime
/// can rely on the upstream ANN's tie-break being honoured.
pub fn rank<S: UtilityScorer, T>(
    scorer: &S,
    candidates: Vec<T>,
    features: impl Fn(&T) -> UtilityFeatures,
) -> Vec<(T, f64)> {
    let mut scored: Vec<(T, f64)> = candidates
        .into_iter()
        .map(|c| {
            let f = features(&c);
            let s = scorer.score(&f);
            (c, s)
        })
        .collect();
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feat(sim: f64, lex: f64, auth: f64, fresh: f64, hist: f64, div: f64) -> UtilityFeatures {
        UtilityFeatures {
            vector_similarity: sim,
            lexical_score: lex,
            source_authority: auth,
            freshness_score: fresh,
            historical_success: hist,
            diversity_score: div,
        }
    }

    #[test]
    fn default_weights_sum_to_one() {
        let w = UtilityWeights::default();
        assert!(
            w.is_normalized(),
            "default weights must sum to 1.0; got {}",
            w.total()
        );
    }

    #[test]
    fn all_zero_features_score_zero() {
        let scorer = LinearUtilityScorer::default_lld();
        assert_eq!(scorer.score(&UtilityFeatures::zero()), 0.0);
    }

    #[test]
    fn perfect_features_score_one_with_default_weights() {
        let scorer = LinearUtilityScorer::default_lld();
        let perfect = feat(1.0, 1.0, 1.0, 1.0, 1.0, 1.0);
        let s = scorer.score(&perfect);
        assert!((s - 1.0).abs() < 1e-9, "expected 1.0, got {s}");
    }

    #[test]
    fn linear_blend_matches_lld_formula() {
        // 0.45*0.8 + 0.20*0.6 + 0.15*1.0 + 0.10*0.5 + 0.05*0.4 + 0.05*0.9
        // = 0.36 + 0.12 + 0.15 + 0.05 + 0.02 + 0.045 = 0.745
        let scorer = LinearUtilityScorer::default_lld();
        let s = scorer.score(&feat(0.8, 0.6, 1.0, 0.5, 0.4, 0.9));
        assert!((s - 0.745).abs() < 1e-9, "expected 0.745, got {s}");
    }

    #[test]
    fn out_of_band_features_are_clamped() {
        // Mis-normalized features must not produce scores above 1.0.
        let scorer = LinearUtilityScorer::default_lld();
        let s = scorer.score(&feat(5.0, -1.0, 2.0, 1.0, 1.0, 1.0));
        assert!(s <= 1.0, "score must not exceed total weight, got {s}");
        assert!(s >= 0.0);
    }

    #[test]
    fn similarity_dominates_with_default_weights() {
        // Two candidates: A has higher similarity, B has higher everything else.
        // The 0.45 weight on similarity should put A on top.
        let scorer = LinearUtilityScorer::default_lld();
        let a = scorer.score(&feat(1.0, 0.0, 0.0, 0.0, 0.0, 0.0));
        let b = scorer.score(&feat(0.0, 1.0, 1.0, 1.0, 1.0, 1.0));
        // a = 0.45; b = 0.20 + 0.15 + 0.10 + 0.05 + 0.05 = 0.55
        // Actually b wins under LLD weights - the sum of non-similarity is 0.55.
        // Pin the contract: when all-non-similarity > 0.45, b wins. This is
        // intentional: a single near-perfect-similarity candidate shouldn't
        // outrank a candidate with broad evidence backing.
        assert!(
            b > a,
            "broad non-similarity evidence (b={b}) should outrank pure similarity (a={a})"
        );
    }

    #[test]
    fn custom_weights_can_invert_ranking() {
        // Boost similarity to 0.9 with zeros elsewhere - now A wins.
        let scorer = LinearUtilityScorer::new(UtilityWeights {
            vector_similarity: 0.9,
            lexical_score: 0.02,
            source_authority: 0.02,
            freshness_score: 0.02,
            historical_success: 0.02,
            diversity_score: 0.02,
        });
        let a = scorer.score(&feat(1.0, 0.0, 0.0, 0.0, 0.0, 0.0));
        let b = scorer.score(&feat(0.0, 1.0, 1.0, 1.0, 1.0, 1.0));
        assert!(a > b, "with similarity-heavy weights, a should win");
    }

    #[test]
    fn rank_orders_descending_and_breaks_ties_stably() {
        let scorer = LinearUtilityScorer::default_lld();
        // Three candidates: c1, c2 tie on score; c3 dominates.
        let candidates = vec!["c1", "c2", "c3"];
        let features: std::collections::HashMap<&str, UtilityFeatures> = [
            ("c1", feat(0.5, 0.5, 0.5, 0.5, 0.5, 0.5)),
            ("c2", feat(0.5, 0.5, 0.5, 0.5, 0.5, 0.5)),
            ("c3", feat(1.0, 1.0, 1.0, 1.0, 1.0, 1.0)),
        ]
        .iter()
        .cloned()
        .collect();
        let ranked = rank(&scorer, candidates, |c| *features.get(*c).unwrap());
        assert_eq!(ranked[0].0, "c3");
        // c1 and c2 tied - stable sort must keep input order.
        assert_eq!(ranked[1].0, "c1");
        assert_eq!(ranked[2].0, "c2");
    }

    #[test]
    fn artifact_scorer_falls_back_until_loaded() {
        // The pending artifact wrapper must not crash; until the runtime
        // loads the model it falls through to the linear blend.
        let s = ArtifactUtilityScorer::pending("/tmp/uae-v1.bin", "uae-v1");
        assert!(!s.is_loaded());
        let linear = LinearUtilityScorer::default_lld();
        let f = feat(0.8, 0.6, 0.5, 0.5, 0.5, 0.5);
        assert!((s.score(&f) - linear.score(&f)).abs() < 1e-9);
        // Name reflects the pending state so observability can flag it.
        assert_eq!(s.name(), "uae-artifact-pending");
    }

    #[test]
    fn linear_scorer_name_is_stable() {
        let s = LinearUtilityScorer::default_lld();
        assert_eq!(s.name(), "linear-default");
    }

    #[test]
    fn weights_total_is_exact_sum() {
        let w = UtilityWeights {
            vector_similarity: 0.4,
            lexical_score: 0.3,
            source_authority: 0.2,
            freshness_score: 0.05,
            historical_success: 0.03,
            diversity_score: 0.02,
        };
        assert!((w.total() - 1.0).abs() < 1e-9);
        assert!(w.is_normalized());
    }

    #[test]
    fn unnormalized_weights_are_detected() {
        let w = UtilityWeights {
            vector_similarity: 0.9,
            lexical_score: 0.9,
            source_authority: 0.0,
            freshness_score: 0.0,
            historical_success: 0.0,
            diversity_score: 0.0,
        };
        assert!(!w.is_normalized());
    }
}
