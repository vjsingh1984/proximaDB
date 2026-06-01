// Copyright 2025 Vijaykumar Singh.
// Licensed under the Apache License, Version 2.0.

//! HNSW parameter advisor — formula-driven sizing of `m`,
//! `ef_construction`, and `ef_search` from
//! `(N, k, recall_target, dim, metric)` plus an optional
//! `max_ef_search` latency cap.
//!
//! # Why
//!
//! HNSW recall depends on three knobs:
//! * **`m`** — out-degree per node on the upper layers (graph
//!   connectivity). Larger `m` improves recall but linearly inflates
//!   memory + build time.
//! * **`ef_construction`** — build-time candidate set. Higher values
//!   produce a better graph (lower NN-error during insertion) at
//!   linear build-time cost.
//! * **`ef_search`** — query-time candidate set. Linear-ish trade-off
//!   between recall and query latency.
//!
//! Production HNSW deployments almost never use the library defaults
//! verbatim. The right `(m, ef_search)` tuple depends on **corpus
//! size**, **dimension**, **distance metric**, and the **recall the
//! caller actually needs** — and increasingly on a **latency budget**
//! that bounds how much ef the operator can afford.
//!
//! This module formalises the sizing decision so callers (collection
//! creation, AXIS adaptive engine, route-health, recall-tune,
//! recluster, RL planner training) can ask the same question and get
//! the same answer.
//!
//! # Formula
//!
//! Closed-form fit against measured m=16 / m=32 / m=48 sweeps at
//! N=100K, dim=128, cosine, k=10:
//!
//! ```text
//!   recall(m, ef, N, k) = ceiling(m) - A(m) · exp(-β · ef / N_factor)
//!   N_factor(N, k)      = k · log₂(N) · log₂(max(2, N / 1000))
//!   β                   ≈ 3.7
//! ```
//!
//! | m  | ceiling(m) | A(m)   | source                                  |
//! |----|-----------:|-------:|-----------------------------------------|
//! | 8  |       0.65 | 0.500  | extrapolated (no measured data)         |
//! | 16 |       0.80 | 0.420  | matches m=16 ceiling ~0.78 in sweep     |
//! | 32 |       1.00 | 0.195  | anchor — verified within 0.002 across   |
//! |    |            |        | all 4 m=32 measured points              |
//! | 48 |       1.00 | 0.019  | from m=48 sweep ef=400 → 0.995          |
//!
//! Inverted to size ef for a given recall target:
//!
//! ```text
//!   ef(m, R, N, k) = N_factor · ln(A(m) / max(ε, ceiling(m) - R)) / β
//! ```
//!
//! `ε = 1e-6` guards against `R ≥ ceiling(m)` (caller asked for recall
//! the graph fundamentally can't deliver — formula returns a very
//! large ef which then jams against the [`EF_SEARCH_MAX`] ceiling and
//! signals "switch to exact search").
//!
//! Untabulated `m` values (e.g. `m=24` from a dim>512 bonus on `m=16`)
//! pick the next lower **tabulated** tier's `ceiling` + `A` — the
//! +8 dim bonus is a graph-quality bump but not a tier promotion.
//!
//! # Sizing decision tree
//!
//! 1. **`m`** by recall tier + high-dim bonus:
//!    * `r < 0.75`         → 8
//!    * `0.75 ≤ r < 0.85`  → 16  (capped at ~0.78 in practice)
//!    * `0.85 ≤ r < 0.97`  → 32
//!    * `r ≥ 0.97`         → 48
//!    * if `dim > 512`     → `+8` (high-D connectivity bump)
//!
//! 2. **`ef_construction`** = `max(100, 8 · m)`. The 8× ratio is
//!    the Malkov-Yashunin paper's recommended floor for stable
//!    build quality.
//!
//! 3. **`ef_search`** = `ef_for_recall(m, r, N, k)`, clamped to
//!    `[max(k, EF_SEARCH_MIN), EF_SEARCH_MAX]`. DotProduct gets a
//!    +15 % bump for unbounded raw scores.
//!
//! 4. **Latency cap**: if the caller supplied
//!    `HnswSizingInput.max_ef_search = Some(cap)` and the advised
//!    `ef_search > cap`, the advisor **clamps** to `cap` and reports:
//!    * `clamped_by_max_ef = true`
//!    * `projected_recall_if_clamped = Some(recall_for_ef(m, cap, N, k))`
//!
//!    The clamping is **honest**: the system honors the operator's
//!    latency ceiling and surfaces the resulting recall on the
//!    route-health endpoint — operators see the conflict and choose
//!    (raise the cap, bump m via /recluster, or accept the
//!    projected recall).
//!
//! # Empirically observed: m=16 saturation + m=48 fast convergence
//!
//! Three sweeps anchor the formula:
//!
//! | ef   | m=16 recall | m=32 recall | m=48 recall |
//! |------|-------------|-------------|-------------|
//! | 100  | 0.575       | (not run)   | (not run)   |
//! | 200  | 0.575       | 0.900       | (not run)   |
//! | 300  | 0.575       | (not run)   | (not run)   |
//! | 400  | (sat)       | 0.950       | 0.995       |
//! | 500  | 0.765       | (not run)   | (not run)   |
//! | 600  | (sat)       | 0.975       | 1.000 ← sat |
//! | 900  | (sat)       | 0.990       | 1.000       |
//! | 1200 | n/a         | n/a         | 1.000       |
//! | 1600 | n/a         | n/a         | 1.000       |
//!
//! Two consequences encoded in the formula:
//! 1. **m=16 plateaus** around recall 0.78 — `ceiling(16) = 0.80` in
//!    the table. The advisor routes any `r ≥ 0.85` to m=32 because
//!    m=16 simply cannot deliver.
//! 2. **m=48 saturates fast** — `A(48) = 0.019` so the exponential
//!    decays to near-zero by ef=400. The advisor stops over-
//!    provisioning at m=48 (the pre-formula table asked for ef≈900
//!    to hit recall=0.99 when the data shows ef=400 already does it).
//!
//! # Future
//!
//! This is the static formula baseline. The RL planner will
//! eventually train a parametric model on per-collection
//! `(sized_ef, observed_recall, observed_latency)` tuples and supplant
//! these constants. Until then, this is the cold-start oracle the
//! planner uses for its prior.

use crate::compute::distance_computation::DistanceMetric;

/// Inputs to the HNSW sizing decision. All fields except
/// `max_ef_search` are required — there is no "default" mode because
/// the right answer depends on every one of these.
#[derive(Debug, Clone, Copy)]
pub struct HnswSizingInput {
    /// Expected corpus size at steady state, in number of vectors.
    pub vector_count: u64,
    /// Top-k results the caller will request per query.
    pub top_k: u32,
    /// Recall target in `[0.0, 1.0]`. Values outside the calibration
    /// range (below 0.50 or above 0.999) clamp to the nearest entry.
    pub recall_target: f32,
    /// Vector dimensionality.
    pub dimension: u32,
    /// Distance metric the collection uses.
    pub distance_metric: DistanceMetric,
    /// Optional latency-budget cap on `ef_search`. When set, the
    /// advisor never recommends an ef above this value — instead it
    /// clamps and reports the projected recall at the clamped ef on
    /// `HnswSizingOutput`. `None` lets the formula pick freely.
    pub max_ef_search: Option<u32>,
}

/// Output of the sizing decision — the three knobs callers need, plus
/// a rationale string for EXPLAIN / structured logs and two flags for
/// the latency-budget clamp.
#[derive(Debug, Clone)]
pub struct HnswSizingOutput {
    pub m: u32,
    pub ef_construction: u32,
    pub ef_search: u32,
    /// True when the formula's advised ef was clamped down to the
    /// caller's `max_ef_search` cap. Surfaces on route-health as
    /// `recall_drift.clamped_by_max_ef`.
    pub clamped_by_max_ef: bool,
    /// When `clamped_by_max_ef = true`, the recall the index will
    /// actually deliver at the clamped ef — typically lower than the
    /// caller's `recall_target`. `None` when no clamp happened.
    /// Surfaces on route-health as
    /// `recall_drift.projected_recall_at_clamped_ef`.
    pub projected_recall_if_clamped: Option<f32>,
    /// One-line free-text explaining which tier the inputs landed in
    /// — written for operator dashboards, not for parsing.
    pub rationale: String,
}

/// Ceiling on `ef_search`. Beyond this, an exact SIMD flat scan
/// typically beats HNSW on both recall and latency, so we'd rather
/// the caller switch search modes than spin ef arbitrarily high.
pub const EF_SEARCH_MAX: u32 = 2048;

/// Floor on `ef_search`. The layer-0 greedy expansion needs enough
/// breadth to escape local minima; below ~16 candidates HNSW degrades
/// to roughly random retrieval.
pub const EF_SEARCH_MIN: u32 = 16;

/// Calibration constant in the exponential recall model. Fits all
/// four measured m=32 data points within 5 % residual.
const BETA: f64 = 3.7;

/// Guard for the inverse formula when caller asks for recall at or
/// above the graph's ceiling.
const EPSILON: f64 = 1e-6;

/// DotProduct gets a +15 % ef bump for unbounded raw-score variance.
const DOTPRODUCT_FACTOR: f64 = 1.15;

/// Per-m graph-quality ceiling. Values above this are unreachable
/// regardless of ef — the graph topology limits recall. Picks the
/// nearest **tabulated** tier (≤ requested m) so a dim-bonus tier
/// like m=24 (m=16 + 8) inherits the m=16 ceiling correctly.
fn ceiling_of_m(m: u32) -> f64 {
    if m >= 32 {
        1.0
    } else if m >= 16 {
        0.80
    } else {
        0.65
    }
}

/// Per-m amplitude of the recall-vs-ef exponential approach. Smaller
/// `A(m)` means the curve hits its ceiling faster (denser graphs
/// converge with less ef). Same nearest-lower-tier selection as
/// `ceiling_of_m`.
///
/// The `m=40` step exists because that's exactly the value the
/// +8 high-dim bonus produces from the m=32 tier — and an in-repo
/// sweep at N=100K measured it: ef=200 → 0.960, ef=400 → 0.975,
/// ef=600 → 0.990. Fitted A(40) ≈ 0.096.
fn a_of_m(m: u32) -> f64 {
    if m >= 48 {
        0.019
    } else if m >= 40 {
        0.096
    } else if m >= 32 {
        0.195
    } else if m >= 16 {
        0.420
    } else {
        0.500
    }
}

/// `N_factor = k · log₂(N) · log₂(max(2, N/1000))` — the "effective
/// corpus difficulty" that scales ef_search super-linearly with N.
fn n_factor(vector_count: u64, top_k: u32) -> f64 {
    let n = vector_count.max(2) as f64;
    let log2_n = n.log2();
    let log2_ratio = (n / 1000.0).max(2.0).log2();
    (top_k.max(1) as f64) * log2_n * log2_ratio
}

/// Forward: predict the recall a given `(m, ef, N, k)` will deliver.
/// Used by the route-health surface to compute
/// `projected_recall_at_clamped_ef` and by the calibration tabulator.
pub fn recall_for_ef(m: u32, ef_search: u32, vector_count: u64, top_k: u32) -> f32 {
    let ceil = ceiling_of_m(m);
    let a = a_of_m(m);
    let nf = n_factor(vector_count, top_k);
    let raw = ceil - a * (-BETA * (ef_search as f64) / nf).exp();
    raw.clamp(0.0, ceil) as f32
}

/// Inverse: the ef the formula thinks delivers `recall_target` at
/// `(m, N, k)`. Returned value already factors in the
/// `[EF_SEARCH_MIN, EF_SEARCH_MAX]` clamp and `top_k` floor.
pub fn ef_for_recall(
    m: u32,
    recall_target: f32,
    vector_count: u64,
    top_k: u32,
) -> u32 {
    let r = recall_target.clamp(0.0, 0.99999) as f64;
    let ceil = ceiling_of_m(m);
    let a = a_of_m(m);
    let nf = n_factor(vector_count, top_k);

    // Headroom above the requested recall vs the per-m ceiling.
    // If r >= ceiling(m), this collapses to EPSILON → very large ef
    // → clamped to EF_SEARCH_MAX → operator should switch to exact
    // search.
    let headroom = (ceil - r).max(EPSILON);
    let raw_ef = nf * (a / headroom).ln() / BETA;

    let floor = EF_SEARCH_MIN.max(top_k);
    (raw_ef.ceil() as i64)
        .max(floor as i64)
        .min(EF_SEARCH_MAX as i64) as u32
}

/// Run the heuristic. See module docs for the decision tree.
pub fn advise_hnsw_params(input: HnswSizingInput) -> HnswSizingOutput {
    let r = input.recall_target.clamp(0.50, 0.999);

    // (1) m — picked by recall tier, with high-dim bonus. Tier
    // boundaries set by the m=16 saturation finding (commit ca2d5620f).
    let mut m: u32 = if r < 0.75 {
        8
    } else if r < 0.85 {
        16
    } else if r < 0.97 {
        32
    } else {
        48
    };
    let m_dim_bonus = if input.dimension > 512 { 8 } else { 0 };
    m += m_dim_bonus;

    // (2) ef_construction = max(100, 8 * m).
    let ef_construction = (8 * m).max(100);

    // (3) ef_search via the inverse formula.
    let mut ef_search = ef_for_recall(m, r, input.vector_count, input.top_k);

    // (3a) DotProduct ef bump.
    if matches!(input.distance_metric, DistanceMetric::DotProduct) {
        let bumped = (ef_search as f64 * DOTPRODUCT_FACTOR).ceil() as u32;
        ef_search = bumped.min(EF_SEARCH_MAX);
    }

    // (4) Latency-budget clamp.
    let (clamped_by_max_ef, projected_recall_if_clamped) = match input.max_ef_search {
        Some(cap) if ef_search > cap => {
            ef_search = cap.max(EF_SEARCH_MIN.max(input.top_k));
            let projected = recall_for_ef(m, ef_search, input.vector_count, input.top_k);
            (true, Some(projected))
        }
        _ => (false, None),
    };

    let rationale = format!(
        "tier r={:.3} → m={} (dim_bonus={}), efc={}, ef={} \
         (n_factor={:.0}, ceiling={:.2}, A={:.3}, metric={:?}{}{}",
        r,
        m,
        m_dim_bonus,
        ef_construction,
        ef_search,
        n_factor(input.vector_count, input.top_k),
        ceiling_of_m(m),
        a_of_m(m),
        input.distance_metric,
        match projected_recall_if_clamped {
            Some(p) => format!(", clamped @ projected_recall={:.3}", p),
            None => String::new(),
        },
        ")",
    );

    HnswSizingOutput {
        m,
        ef_construction,
        ef_search,
        clamped_by_max_ef,
        projected_recall_if_clamped,
        rationale,
    }
}

// ───── AnnIndexAdvisor trait implementation ──────────────────
//
// Thin wrapper over the pure functions above so the polymorphic
// AnnSelector can dispatch HNSW alongside other algorithms. The
// trait's `AnnAdvisorInput` carries declared budgets
// (max_query_latency_ms, max_memory_mb) that this impl maps to
// HNSW-specific knobs.

use crate::index::axis::management::ann_advisor::{
    AnnAdvisorInput, AnnAdvisorOutput, AnnIndexAdvisor, SupportedAlgorithm,
};
use crate::index::axis::types::IndexAlgorithm;

/// HNSW impl of [`AnnIndexAdvisor`]. Constructed via [`Self::new`]
/// — no state, no config.
pub struct HnswIndexAdvisor;

impl HnswIndexAdvisor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for HnswIndexAdvisor {
    fn default() -> Self {
        Self::new()
    }
}

/// Coarse cost model: HNSW per-query latency ≈ `ef_search · 0.5μs`.
/// Calibrated from the matrix bench's measured p50 latencies
/// (m=32 ef=400 at N=100K → ~200μs); the actual constant varies
/// with dim and CPU but 0.5μs is a defensible mid-range default.
/// Used to translate `max_query_latency_ms` → `max_ef_search`.
const HNSW_US_PER_EF_CANDIDATE: f64 = 0.5;

impl AnnIndexAdvisor for HnswIndexAdvisor {
    fn algorithm(&self) -> SupportedAlgorithm {
        SupportedAlgorithm::Hnsw
    }

    fn advise(&self, input: &AnnAdvisorInput) -> Option<AnnAdvisorOutput> {
        // Map declared budgets → HNSW-specific knobs.
        let max_ef_search = input.max_query_latency_ms.map(|ms| {
            ((ms * 1000.0) / HNSW_US_PER_EF_CANDIDATE)
                .ceil()
                .clamp(EF_SEARCH_MIN as f64, EF_SEARCH_MAX as f64) as u32
        });

        // Delegate to the existing pure-function advisor.
        let sized = advise_hnsw_params(HnswSizingInput {
            vector_count: input.vector_count,
            top_k: input.top_k,
            recall_target: input.recall_target,
            dimension: input.dimension,
            distance_metric: input.distance_metric,
            max_ef_search,
        });

        // Estimate memory: m · 4 bytes/edge · dim · N + raw vectors.
        // HNSW edges + raw fp32 vectors dominate; ignore layer
        // overhead at this granularity.
        let n = input.vector_count.max(1) as f64;
        let dim = input.dimension.max(1) as f64;
        let edge_bytes = (sized.m as f64) * 4.0 * n;
        let vector_bytes = dim * 4.0 * n;
        let estimated_memory_mb = (edge_bytes + vector_bytes) / (1024.0 * 1024.0);

        // Estimate per-query work: ef_search candidates inspected
        // at layer 0 is the dominant cost.
        let estimated_per_query_work = sized.ef_search as u64;

        let algorithm = IndexAlgorithm::HNSW {
            m: sized.m,
            ef_construction: sized.ef_construction,
            ef_search: sized.ef_search,
            max_elements: input.vector_count.max(1024) as usize,
        };

        Some(AnnAdvisorOutput {
            algorithm,
            kind: SupportedAlgorithm::Hnsw,
            clamped_by_budget: sized.clamped_by_max_ef,
            projected_recall: sized.projected_recall_if_clamped,
            estimated_memory_mb,
            estimated_per_query_work,
            rationale: sized.rationale,
        })
    }

    fn recall_for(
        &self,
        algorithm: &IndexAlgorithm,
        vector_count: u64,
        top_k: u32,
    ) -> Option<f32> {
        match algorithm {
            IndexAlgorithm::HNSW { m, ef_search, .. } => {
                Some(recall_for_ef(*m, *ef_search, vector_count, top_k))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cosine_in(n: u64, k: u32, r: f32) -> HnswSizingInput {
        HnswSizingInput {
            vector_count: n,
            top_k: k,
            recall_target: r,
            dimension: 128,
            distance_metric: DistanceMetric::Cosine,
            max_ef_search: None,
        }
    }

    // ───── m-tier decision tree ──────────────────────────────────

    #[test]
    fn m_grows_with_recall_target() {
        let very_lo = advise_hnsw_params(cosine_in(10_000, 10, 0.70));
        let lo = advise_hnsw_params(cosine_in(10_000, 10, 0.80));
        let mid = advise_hnsw_params(cosine_in(10_000, 10, 0.90));
        let hi = advise_hnsw_params(cosine_in(10_000, 10, 0.95));
        let max = advise_hnsw_params(cosine_in(10_000, 10, 0.99));
        assert_eq!(very_lo.m, 8);
        assert_eq!(lo.m, 16);
        assert_eq!(mid.m, 32);
        assert_eq!(hi.m, 32);
        assert_eq!(max.m, 48);
    }

    #[test]
    fn m_high_dim_bonus_applies() {
        let mut input = cosine_in(10_000, 10, 0.95);
        input.dimension = 1024;
        let out = advise_hnsw_params(input);
        // 32 (tier) + 8 (high-dim bonus)
        assert_eq!(out.m, 40);
    }

    #[test]
    fn ef_construction_is_max_of_100_and_8m() {
        let small_m = advise_hnsw_params(cosine_in(10_000, 10, 0.70)); // m=8
        let big_m = advise_hnsw_params(cosine_in(10_000, 10, 0.99)); // m=48
        assert_eq!(small_m.ef_construction, 100); // max(100, 8*8=64)
        assert_eq!(big_m.ef_construction, 384); // max(100, 8*48=384)
    }

    // ───── formula correctness vs measured data ────────────────

    #[test]
    fn recall_for_ef_matches_m32_sweep() {
        // N=100K, cosine, k=10, m=32. Measured: ef → recall:
        //   200 → 0.900   400 → 0.950   600 → 0.975   900 → 0.990
        // Allow ±0.005 absolute (the anchor).
        let checks = [(200u32, 0.900f32), (400, 0.950), (600, 0.975), (900, 0.990)];
        for (ef, expected) in checks {
            let got = recall_for_ef(32, ef, 100_000, 10);
            assert!(
                (got - expected).abs() < 0.005,
                "m=32 ef={}: predicted {:.4}, expected {:.4}",
                ef,
                got,
                expected
            );
        }
    }

    #[test]
    fn recall_for_ef_matches_m48_anchor() {
        // m=48 ef=400 → recall=0.995 (single measured point, ±0.005).
        let got = recall_for_ef(48, 400, 100_000, 10);
        assert!(
            (got - 0.995).abs() < 0.005,
            "m=48 ef=400: predicted {:.4}, expected 0.995",
            got
        );
    }

    #[test]
    fn recall_for_ef_matches_m40_sweep() {
        // m=40 sweep at N=100K (the +8 dim-bonus tier):
        //   ef=200 → 0.960, ef=400 → 0.975, ef=600 → 0.990
        // ±0.015 tolerance — fit accepts noise at the asymptote.
        let checks = [
            (200u32, 0.960f32, 0.015f32),
            (400, 0.975, 0.015),
            (600, 0.990, 0.010),
        ];
        for (ef, expected, tol) in checks {
            let got = recall_for_ef(40, ef, 100_000, 10);
            assert!(
                (got - expected).abs() < tol,
                "m=40 ef={}: predicted {:.4}, expected {:.4} (±{})",
                ef,
                got,
                expected,
                tol
            );
        }
    }

    #[test]
    fn recall_for_ef_saturates_at_m48() {
        // m=48 ef≥600 → recall ≈ 1.000 per sweep. Formula
        // asymptotically approaches ceiling(48)=1.0; the tiny
        // residual at ef=600 (≈0.003) is the exponential's tail —
        // operationally indistinguishable from saturation.
        let got_600 = recall_for_ef(48, 600, 100_000, 10);
        let got_900 = recall_for_ef(48, 900, 100_000, 10);
        let got_1600 = recall_for_ef(48, 1600, 100_000, 10);
        assert!(got_600 >= 0.995, "m=48 ef=600 should be ~1.0, got {}", got_600);
        assert!(got_900 >= 0.999);
        assert!(got_1600 >= 0.9999);
    }

    #[test]
    fn recall_for_ef_respects_m16_ceiling() {
        // m=16 caps at 0.80 regardless of ef.
        for ef in [100u32, 200, 500, 1000, 2048] {
            let got = recall_for_ef(16, ef, 100_000, 10);
            assert!(
                got <= 0.80 + 1e-4,
                "m=16 ef={} should not exceed ceiling 0.80, got {}",
                ef,
                got
            );
        }
    }

    #[test]
    fn ef_for_recall_inverts_recall_for_ef() {
        // Round-trip: ef_for_recall(m, recall_for_ef(m, ef, ...)) ≈ ef.
        // Allow ±1 from integer ceiling.
        for m in [32u32, 48] {
            for ef in [200u32, 400, 600, 900] {
                let r = recall_for_ef(m, ef, 100_000, 10);
                if r >= ceiling_of_m(m) as f32 - 0.001 {
                    continue; // saturated — inverse loses precision
                }
                let inverted = ef_for_recall(m, r, 100_000, 10);
                let diff = (inverted as i64 - ef as i64).abs();
                assert!(
                    diff <= 2,
                    "m={} ef={} → r={:.4} → ef_inv={} (diff {})",
                    m,
                    ef,
                    r,
                    inverted,
                    diff
                );
            }
        }
    }

    #[test]
    fn ef_for_recall_handles_unreachable_target() {
        // recall=0.90 at m=16 (ceiling 0.80) → unreachable. The
        // formula returns EF_SEARCH_MAX as the "switch to exact"
        // signal.
        let ef = ef_for_recall(16, 0.90, 100_000, 10);
        assert_eq!(ef, EF_SEARCH_MAX);
    }

    // ───── monotonicity ────────────────────────────────────────

    #[test]
    fn ef_search_grows_with_n() {
        let small = advise_hnsw_params(cosine_in(10_000, 10, 0.90));
        let mid = advise_hnsw_params(cosine_in(100_000, 10, 0.90));
        let big = advise_hnsw_params(cosine_in(1_000_000, 10, 0.90));
        assert!(small.ef_search < mid.ef_search);
        assert!(mid.ef_search < big.ef_search);
    }

    #[test]
    fn ef_search_grows_with_recall_target() {
        let lo = advise_hnsw_params(cosine_in(100_000, 10, 0.85));
        let mid = advise_hnsw_params(cosine_in(100_000, 10, 0.92));
        let hi = advise_hnsw_params(cosine_in(100_000, 10, 0.95));
        let max = advise_hnsw_params(cosine_in(100_000, 10, 0.99));
        // Monotonic within tiers; tier-crossing flips m and lowers A
        // so absolute ef may dip — assert recall_target moves the
        // joint (m, ef) sizing upward.
        assert!(lo.ef_search < mid.ef_search);
        // hi (r=0.95) is m=32; max (r=0.99) is m=48 (denser graph,
        // lower A) — ef can actually be LOWER at higher recall when
        // the m promotion buys all the headroom. We assert the joint
        // (m, ef) is monotonic by m instead.
        assert!(hi.m <= max.m);
    }

    #[test]
    fn ef_search_respects_floor_and_top_k() {
        let input = HnswSizingInput {
            vector_count: 500,
            top_k: 64,
            recall_target: 0.80,
            dimension: 128,
            distance_metric: DistanceMetric::Cosine,
            max_ef_search: None,
        };
        let out = advise_hnsw_params(input);
        assert!(out.ef_search >= 64);
    }

    #[test]
    fn ef_search_respects_ceiling() {
        let input = HnswSizingInput {
            vector_count: 100_000_000,
            top_k: 100,
            recall_target: 0.995,
            dimension: 1024,
            distance_metric: DistanceMetric::Cosine,
            max_ef_search: None,
        };
        let out = advise_hnsw_params(input);
        assert!(out.ef_search <= EF_SEARCH_MAX);
    }

    // ───── metric adjustments ───────────────────────────────────

    #[test]
    fn dotproduct_pays_penalty() {
        let mut cos = cosine_in(100_000, 10, 0.95);
        let mut dot = cos;
        cos.distance_metric = DistanceMetric::Cosine;
        dot.distance_metric = DistanceMetric::DotProduct;
        let cos_out = advise_hnsw_params(cos);
        let dot_out = advise_hnsw_params(dot);
        assert!(dot_out.ef_search > cos_out.ef_search);
        let ratio = dot_out.ef_search as f64 / cos_out.ef_search as f64;
        assert!(
            ratio > 1.10 && ratio < 1.25,
            "dot/cos ef ratio {} outside [1.10, 1.25]",
            ratio
        );
    }

    // ───── m-saturation regression ──────────────────────────────

    #[test]
    fn recall_target_above_saturation_picks_higher_m() {
        let out_85 = advise_hnsw_params(cosine_in(100_000, 10, 0.85));
        assert_eq!(
            out_85.m, 32,
            "recall_target 0.85 must land on m=32 — m=16 caps at ~0.78"
        );
        let out_95 = advise_hnsw_params(cosine_in(100_000, 10, 0.95));
        assert_eq!(out_95.m, 32);
        let out_99 = advise_hnsw_params(cosine_in(100_000, 10, 0.99));
        assert_eq!(out_99.m, 48);
    }

    // ───── latency budget (max_ef_search) ──────────────────────

    #[test]
    fn max_ef_search_clamps_advised_ef() {
        let mut input = cosine_in(100_000, 10, 0.95);
        input.max_ef_search = Some(300);
        let out = advise_hnsw_params(input);
        assert!(out.clamped_by_max_ef);
        assert_eq!(out.ef_search, 300);
        let projected = out
            .projected_recall_if_clamped
            .expect("projected recall must populate on clamp");
        assert!(
            projected < 0.95,
            "projected_recall {} must be below the unclamped target 0.95",
            projected
        );
        assert!(
            projected > 0.85,
            "projected_recall {} should still be meaningful at ef=300, m=32",
            projected
        );
    }

    #[test]
    fn max_ef_search_unset_does_not_clamp() {
        let out = advise_hnsw_params(cosine_in(100_000, 10, 0.95));
        assert!(!out.clamped_by_max_ef);
        assert!(out.projected_recall_if_clamped.is_none());
    }

    #[test]
    fn max_ef_search_above_advised_is_noop() {
        let mut input = cosine_in(100_000, 10, 0.95);
        input.max_ef_search = Some(2000); // well above the ~405 advised
        let out = advise_hnsw_params(input);
        assert!(!out.clamped_by_max_ef);
        assert!(out.projected_recall_if_clamped.is_none());
        assert!(out.ef_search < 2000);
    }

    // ───── rationale + observability ───────────────────────────

    #[test]
    fn rationale_is_populated_for_observability() {
        let out = advise_hnsw_params(cosine_in(100_000, 10, 0.95));
        assert!(out.rationale.contains("m="));
        assert!(out.rationale.contains("efc="));
        assert!(out.rationale.contains("ef="));
        assert!(out.rationale.contains("metric="));
        assert!(out.rationale.contains("n_factor="));
        assert!(out.rationale.contains("ceiling="));
    }

    // ───── AnnIndexAdvisor trait impl ──────────────────────────

    #[test]
    fn trait_impl_matches_free_function() {
        // The trait impl is a thin wrapper over advise_hnsw_params —
        // pin that the two return identical sizing.
        let advisor = HnswIndexAdvisor::new();
        let input = AnnAdvisorInput {
            vector_count: 100_000,
            top_k: 10,
            recall_target: 0.95,
            dimension: 128,
            distance_metric: DistanceMetric::Cosine,
            max_query_latency_ms: None,
            max_memory_mb: None,
            binary_rerank_allowed: false,
        };
        let out = advisor.advise(&input).expect("HNSW always responds");
        let direct = advise_hnsw_params(HnswSizingInput {
            vector_count: input.vector_count,
            top_k: input.top_k,
            recall_target: input.recall_target,
            dimension: input.dimension,
            distance_metric: input.distance_metric,
            max_ef_search: None,
        });
        match out.algorithm {
            IndexAlgorithm::HNSW {
                m,
                ef_construction,
                ef_search,
                ..
            } => {
                assert_eq!(m, direct.m);
                assert_eq!(ef_construction, direct.ef_construction);
                assert_eq!(ef_search, direct.ef_search);
            }
            other => panic!("expected HNSW variant, got {:?}", other),
        }
        assert_eq!(out.kind, SupportedAlgorithm::Hnsw);
    }

    #[test]
    fn trait_impl_translates_latency_budget_to_ef_cap() {
        // max_query_latency_ms=0.2ms → max_ef_search ≈ 400.
        let advisor = HnswIndexAdvisor::new();
        let input = AnnAdvisorInput {
            vector_count: 100_000,
            top_k: 10,
            recall_target: 0.99, // would unconstrained advise ef≈905
            dimension: 128,
            distance_metric: DistanceMetric::Cosine,
            max_query_latency_ms: Some(0.2), // 200μs cap → ~400 ef
            max_memory_mb: None,
            binary_rerank_allowed: false,
        };
        let out = advisor.advise(&input).unwrap();
        // The actual ef may be slightly under 400 due to the
        // ceil()→u32 conversion at exactly 400; accept anything ≤ 400.
        if let IndexAlgorithm::HNSW { ef_search, .. } = out.algorithm {
            assert!(
                ef_search <= 400,
                "ef_search {} should be clamped at or below 400 by latency budget",
                ef_search
            );
        }
        assert!(out.clamped_by_budget, "latency budget should clamp");
        assert!(out.projected_recall.is_some());
    }

    #[test]
    fn trait_impl_recall_for_returns_none_on_wrong_variant() {
        let advisor = HnswIndexAdvisor::new();
        let ivf_spec = IndexAlgorithm::IVF {
            nlist: 316,
            nprobe: 20,
            quantizer: None,
        };
        assert!(
            advisor.recall_for(&ivf_spec, 100_000, 10).is_none(),
            "HNSW advisor must decline non-HNSW algorithm specs"
        );
    }

    #[test]
    fn trait_impl_recall_for_matches_free_function() {
        let advisor = HnswIndexAdvisor::new();
        let spec = IndexAlgorithm::HNSW {
            m: 32,
            ef_construction: 256,
            ef_search: 400,
            max_elements: 100_000,
        };
        let got = advisor
            .recall_for(&spec, 100_000, 10)
            .expect("HNSW advisor handles HNSW spec");
        let direct = recall_for_ef(32, 400, 100_000, 10);
        assert_eq!(got, direct);
    }

    #[test]
    fn trait_impl_estimates_memory_for_balance_test() {
        // At m=32, dim=128, N=100K:
        //   edges = 32 · 4 · 100K = 12.8 MB
        //   vectors = 128 · 4 · 100K = 48.8 MB
        //   total ≈ 61.6 MB. Pin within ±5MB.
        let advisor = HnswIndexAdvisor::new();
        let input = AnnAdvisorInput {
            vector_count: 100_000,
            top_k: 10,
            recall_target: 0.95,
            dimension: 128,
            distance_metric: DistanceMetric::Cosine,
            max_query_latency_ms: None,
            max_memory_mb: None,
            binary_rerank_allowed: false,
        };
        let out = advisor.advise(&input).unwrap();
        assert!(
            (out.estimated_memory_mb - 61.6).abs() < 5.0,
            "memory estimate {} should be ~61.6 MB ±5",
            out.estimated_memory_mb
        );
    }

    #[test]
    fn rationale_mentions_clamp_when_active() {
        let mut input = cosine_in(100_000, 10, 0.95);
        input.max_ef_search = Some(300);
        let out = advise_hnsw_params(input);
        assert!(
            out.rationale.contains("clamped"),
            "rationale should disclose clamp when active: {}",
            out.rationale
        );
    }

    /// Dump the full sizing table for k=10, dim=128, cosine. Useful
    /// for operator docs and quick "what would the advisor pick for
    /// my collection?" lookups. Run with:
    ///   cargo test --lib --
    ///     hnsw_param_advisor::tests::dump_sizing_table -- --ignored --nocapture
    #[test]
    #[ignore]
    fn dump_sizing_table() {
        let sizes: &[(u64, &str)] = &[
            (10_000, "10K"),
            (100_000, "100K"),
            (1_000_000, "1M"),
            (10_000_000, "10M"),
            (100_000_000, "100M"),
        ];
        let targets = [0.80f32, 0.85, 0.90, 0.92, 0.95, 0.97, 0.99, 0.995];
        println!("\n{:<8}{:<10}{:>5}{:>6}{:>8}", "N", "recall", "m", "efc", "ef");
        println!("{}", "-".repeat(40));
        for (n, n_label) in sizes {
            for &target in &targets {
                let out = advise_hnsw_params(HnswSizingInput {
                    vector_count: *n,
                    top_k: 10,
                    recall_target: target,
                    dimension: 128,
                    distance_metric: DistanceMetric::Cosine,
                    max_ef_search: None,
                });
                println!(
                    "{:<8}{:<10}{:>5}{:>6}{:>8}",
                    n_label,
                    format!("{:.3}", target),
                    out.m,
                    out.ef_construction,
                    out.ef_search
                );
            }
            println!();
        }
    }
}
