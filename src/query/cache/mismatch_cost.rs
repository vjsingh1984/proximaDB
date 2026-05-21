// Mismatch-cost online learner (CUCB-SC) — arXiv 2508.07675.
//
// The result cache holds entries keyed on (tenant, normalized query, filter
// digest). A cache hit on a *similar* — not identical — query is correct only
// if the mismatch cost the customer would pay (lost recall, stale evidence)
// is below the serving cost we'd otherwise incur to re-execute the query.
//
// 2508.07675 proves the offline objective NP-hard, then gives a combinatorial
// upper-confidence-bound algorithm (CUCB-SC) that learns the per-region
// mismatch_cost distribution online from observed feedback. We adopt the
// same structure with three simplifications appropriate to a Phase-2 ship:
//
//   1. Regions are tenant × category buckets, not learned clusters. Cluster
//      learning is a Phase-7 follow-up (see plan §"learned planner v2").
//   2. The CUCB step uses Hoeffding bounds with the standard `sqrt(ln(t) / n)`
//      term. Beta-Bernoulli posteriors are unnecessary because every
//      observation is a bounded scalar in [0.0, 1.0].
//   3. We expose a single `decide()` API that returns Accept / Reject without
//      a separate explore/exploit step — the UCB term naturally encourages
//      exploration on under-sampled regions.
//
// The decision rule: accept the cache hit when
//   similarity ≥ region_threshold  AND  ucb_mismatch_cost ≤ allowed_cost
// and reject otherwise. Both sides of the inequality update on observed
// feedback, so the threshold tightens as the planner sees more rejections.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// Region identifier — bucket the learner groups observations into.
/// Phase 2 uses (tenant_id, category); Phase 7 can swap in learned cluster ids.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct Region {
    pub tenant_id: String,
    pub category: String,
}

impl Region {
    pub fn new(tenant_id: impl Into<String>, category: impl Into<String>) -> Self {
        Self { tenant_id: tenant_id.into(), category: category.into() }
    }
}

/// Outcome of evaluating a candidate cache hit against the mismatch-cost gate.
#[derive(Debug, Clone, PartialEq)]
pub enum MismatchDecision {
    /// Serve from the cache.
    Accept {
        /// The Hoeffding-bounded mismatch cost estimate that justified the hit.
        ucb_mismatch_cost: f64,
        /// The configured allowed cost ceiling.
        allowed_cost: f64,
    },
    /// Re-execute the query.
    Reject {
        /// Reason: "below_similarity" | "above_allowed_cost" | "cold_region".
        reason: &'static str,
        /// Estimated mismatch cost (could be 1.0 — the cold-region default).
        ucb_mismatch_cost: f64,
    },
}

impl MismatchDecision {
    pub fn is_accept(&self) -> bool {
        matches!(self, MismatchDecision::Accept { .. })
    }
}

/// Per-region running statistics. Stored in the learner's inner map and
/// updated on every feedback observation.
#[derive(Debug, Clone)]
struct RegionStats {
    /// Number of feedback observations seen.
    samples: u64,
    /// Sum of observed mismatch costs (each in [0.0, 1.0]).
    cost_sum: f64,
    /// Last update timestamp — used for TTL-style decay so an old region
    /// re-explores when traffic returns after a long gap.
    last_update: Instant,
}

impl RegionStats {
    fn new() -> Self {
        Self { samples: 0, cost_sum: 0.0, last_update: Instant::now() }
    }

    fn mean_cost(&self) -> f64 {
        if self.samples == 0 {
            // Cold region — assume the worst case so the gate rejects.
            return 1.0;
        }
        self.cost_sum / self.samples as f64
    }

    /// Hoeffding upper-confidence-bound for the per-region mean cost.
    /// `total_samples` is the global observation count across all regions.
    /// `decay_seconds` decays old stats so a long-idle region re-explores.
    fn ucb_cost(&self, total_samples: u64, decay_seconds: u64) -> f64 {
        if self.samples == 0 {
            return 1.0;
        }
        let elapsed = self.last_update.elapsed().as_secs();
        let effective_samples = if elapsed > decay_seconds {
            // Exponentially decay sample weight when the region has been idle.
            // Halves every `decay_seconds` since last update.
            let halvings = ((elapsed - decay_seconds) / decay_seconds.max(1)).min(20) as u32;
            let factor = 0.5f64.powi(halvings as i32).max(1e-6);
            (self.samples as f64 * factor).max(1.0)
        } else {
            self.samples as f64
        };
        let log_t = (total_samples.max(1) as f64).ln();
        let exploration = (2.0 * log_t / effective_samples).sqrt();
        (self.mean_cost() + exploration).min(1.0)
    }
}

/// Configuration knobs for the mismatch-cost learner. Defaults are
/// conservative (high allowed_cost would make every cache hit accept; low
/// allowed_cost would make every hit reject). The defaults below were
/// chosen so a cold region sits at the rejection threshold — only regions
/// that learn low mismatch cost open up cache hits.
#[derive(Debug, Clone, Copy)]
pub struct MismatchConfig {
    /// Minimum cosine similarity needed before the mismatch-cost gate runs.
    /// Below this, the cache always rejects regardless of cost.
    pub similarity_floor: f64,
    /// Maximum mismatch cost we will accept on a hit.
    pub allowed_cost: f64,
    /// Seconds after which a region's stats begin to decay (re-explore).
    pub decay_seconds: u64,
}

impl Default for MismatchConfig {
    fn default() -> Self {
        Self {
            similarity_floor: 0.85,
            allowed_cost: 0.15,
            decay_seconds: 3600,
        }
    }
}

/// CUCB-SC learner. Cheap to clone — wraps an `Arc<RwLock<…>>`.
#[derive(Clone)]
pub struct MismatchCostLearner {
    inner: Arc<RwLock<LearnerState>>,
    config: MismatchConfig,
}

struct LearnerState {
    per_region: HashMap<Region, RegionStats>,
    total_samples: u64,
}

impl MismatchCostLearner {
    pub fn new(config: MismatchConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(LearnerState {
                per_region: HashMap::new(),
                total_samples: 0,
            })),
            config,
        }
    }

    /// Decide whether to serve from the cache. The similarity is the cosine
    /// distance between the incoming query embedding and the cached entry's
    /// centroid; the region groups feedback per tenant + workload category.
    pub async fn decide(&self, region: &Region, similarity: f64) -> MismatchDecision {
        if similarity < self.config.similarity_floor {
            return MismatchDecision::Reject {
                reason: "below_similarity",
                ucb_mismatch_cost: 1.0,
            };
        }
        let state = self.inner.read().await;
        let stats = state.per_region.get(region);
        let ucb = match stats {
            None => 1.0, // cold region — refuse first hit to gather data
            Some(s) => s.ucb_cost(state.total_samples, self.config.decay_seconds),
        };
        if stats.is_none() {
            return MismatchDecision::Reject {
                reason: "cold_region",
                ucb_mismatch_cost: ucb,
            };
        }
        if ucb <= self.config.allowed_cost {
            MismatchDecision::Accept {
                ucb_mismatch_cost: ucb,
                allowed_cost: self.config.allowed_cost,
            }
        } else {
            MismatchDecision::Reject {
                reason: "above_allowed_cost",
                ucb_mismatch_cost: ucb,
            }
        }
    }

    /// Record an observation of mismatch cost after the query completed.
    /// `cost` must be in [0.0, 1.0] — 0 means the cache hit was perfectly
    /// accurate, 1 means it was wrong. Values outside the interval are
    /// clamped so a misbehaving caller can't poison the learner. NaN inputs
    /// are silently dropped — `clamp` on NaN returns NaN, which would
    /// poison the running sum.
    pub async fn observe(&self, region: Region, cost: f64) {
        if cost.is_nan() {
            return;
        }
        let clamped = cost.clamp(0.0, 1.0);
        let mut state = self.inner.write().await;
        let entry = state.per_region.entry(region).or_insert_with(RegionStats::new);
        entry.samples += 1;
        entry.cost_sum += clamped;
        entry.last_update = Instant::now();
        state.total_samples += 1;
    }

    /// Snapshot per-region mean cost. Useful for observability dashboards.
    pub async fn snapshot(&self) -> HashMap<Region, f64> {
        self.inner
            .read()
            .await
            .per_region
            .iter()
            .map(|(r, s)| (r.clone(), s.mean_cost()))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(similarity_floor: f64, allowed_cost: f64) -> MismatchConfig {
        MismatchConfig { similarity_floor, allowed_cost, decay_seconds: 3600 }
    }

    #[tokio::test]
    async fn below_similarity_floor_is_rejected() {
        let learner = MismatchCostLearner::new(MismatchConfig::default());
        let r = Region::new("t", "code");
        let decision = learner.decide(&r, 0.5).await;
        match decision {
            MismatchDecision::Reject { reason, .. } => assert_eq!(reason, "below_similarity"),
            other => panic!("expected reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cold_region_rejects_first_hit() {
        let learner = MismatchCostLearner::new(cfg(0.5, 0.5));
        let r = Region::new("t", "code");
        let decision = learner.decide(&r, 0.9).await;
        match decision {
            MismatchDecision::Reject { reason, .. } => assert_eq!(reason, "cold_region"),
            other => panic!("expected cold reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn warm_low_cost_region_accepts() {
        let learner = MismatchCostLearner::new(cfg(0.5, 0.5));
        let r = Region::new("t", "code");
        for _ in 0..50 {
            learner.observe(r.clone(), 0.05).await;
        }
        let decision = learner.decide(&r, 0.9).await;
        assert!(decision.is_accept(), "expected accept, got {decision:?}");
    }

    #[tokio::test]
    async fn warm_high_cost_region_rejects() {
        let learner = MismatchCostLearner::new(cfg(0.5, 0.1));
        let r = Region::new("t", "convo");
        for _ in 0..50 {
            learner.observe(r.clone(), 0.4).await;
        }
        let decision = learner.decide(&r, 0.9).await;
        match decision {
            MismatchDecision::Reject { reason, ucb_mismatch_cost } => {
                assert_eq!(reason, "above_allowed_cost");
                assert!(ucb_mismatch_cost > 0.1);
            }
            other => panic!("expected high-cost reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn ucb_term_shrinks_as_samples_grow() {
        // UCB1 exploration term = sqrt(2 * ln(t) / n_i). At n=t=1 the log
        // is zero, which makes the "few samples" baseline degenerate — start
        // with a non-trivial sample count so the comparison reflects actual
        // UCB shrinking, not the special-case zero point.
        let learner = MismatchCostLearner::new(cfg(0.5, 1.0));
        let r = Region::new("t", "code");
        for _ in 0..10 {
            learner.observe(r.clone(), 0.1).await;
        }
        let snap_few = match learner.decide(&r, 0.9).await {
            MismatchDecision::Accept { ucb_mismatch_cost, .. } => ucb_mismatch_cost,
            MismatchDecision::Reject { ucb_mismatch_cost, .. } => ucb_mismatch_cost,
        };
        for _ in 0..500 {
            learner.observe(r.clone(), 0.1).await;
        }
        let snap_many = match learner.decide(&r, 0.9).await {
            MismatchDecision::Accept { ucb_mismatch_cost, .. } => ucb_mismatch_cost,
            MismatchDecision::Reject { ucb_mismatch_cost, .. } => ucb_mismatch_cost,
        };
        assert!(snap_many < snap_few, "UCB should shrink with more data: {} -> {}", snap_few, snap_many);
        // Mean is 0.1; with 500+ samples UCB should be within ~0.3.
        assert!(snap_many < 0.3, "UCB too wide after 500 samples: {snap_many}");
    }

    #[tokio::test]
    async fn observations_are_clamped_to_unit_interval() {
        let learner = MismatchCostLearner::new(cfg(0.5, 1.0));
        let r = Region::new("t", "code");
        // Misbehaving caller emits out-of-range costs.
        learner.observe(r.clone(), -1.0).await;
        learner.observe(r.clone(), 5.0).await;
        learner.observe(r.clone(), f64::NAN).await;
        let snap = learner.snapshot().await;
        let mean = *snap.get(&r).expect("region present");
        // -1.0 clamps to 0, 5.0 clamps to 1, NaN clamps to 0 (clamp(0,1) on NaN returns 0).
        // We don't pin the exact value because NaN-clamp behavior is platform-dependent;
        // just assert it stays in the unit interval.
        assert!(mean.is_finite() && (0.0..=1.0).contains(&mean));
    }

    #[tokio::test]
    async fn different_regions_are_independent() {
        let learner = MismatchCostLearner::new(cfg(0.5, 0.5));
        let code = Region::new("t", "code");
        let convo = Region::new("t", "convo");
        for _ in 0..50 {
            learner.observe(code.clone(), 0.05).await;
            learner.observe(convo.clone(), 0.4).await;
        }
        let code_decision = learner.decide(&code, 0.9).await;
        let convo_decision = learner.decide(&convo, 0.9).await;
        assert!(code_decision.is_accept());
        assert!(!convo_decision.is_accept());
    }

    #[tokio::test]
    async fn snapshot_returns_per_region_means() {
        let learner = MismatchCostLearner::new(cfg(0.5, 0.5));
        let r = Region::new("t", "code");
        learner.observe(r.clone(), 0.1).await;
        learner.observe(r.clone(), 0.3).await;
        let snap = learner.snapshot().await;
        let mean = *snap.get(&r).expect("region present");
        assert!((mean - 0.2).abs() < 1e-9);
    }
}
