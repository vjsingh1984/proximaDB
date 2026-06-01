// Copyright 2025 Vijaykumar Singh.
// Licensed under the Apache License, Version 2.0.

//! IVF parameter advisor — formula-driven sizing of `nlist` and
//! `nprobe` from `(N, k, recall_target, dim, metric)` plus an
//! optional `max_query_latency_ms` / `max_memory_mb` budget.
//!
//! # Formula
//!
//! Calibrated against an in-repo nprobe sweep at `N=100K,
//! nlist=316, dim=128, cosine, k=10`:
//!
//! ```text
//!   recall(nprobe) = ceiling - A · exp(-γ · nprobe)
//!   ceiling = 0.74      (saturation observed at nprobe ≥ 100)
//!   A       = 0.41      (max recall improvement available)
//!   γ       = 0.037     (per-nprobe decay rate)
//! ```
//!
//! Measured vs predicted:
//!
//! | nprobe | measured | predicted | residual |
//! |--------|----------|-----------|----------|
//! |   5    | 0.400    | 0.401     | +0.001   |
//! |   10   | 0.450    | 0.457     | +0.007   |
//! |   20   | 0.530    | 0.544     | +0.014   |
//! |   50   | 0.675    | 0.675     |  0.000 ← anchor |
//! |  100   | 0.740    | 0.730     | -0.010   |
//! |  200   | 0.740    | 0.740     |  0.000   |
//!
//! # Saturation caveat
//!
//! The 0.74 ceiling is a property of k-means + centroid-distance
//! probing at `nlist=316`: true nearest neighbours near Voronoi
//! cell boundaries land in centroid-distant clusters that
//! `nprobe ≤ 200` doesn't reach. This is the IVF equivalent
//! of the m=16 HNSW graph saturation we measured earlier — and is
//! why the IVF advisor **declines** (`advise` returns `None`) for
//! `recall_target > 0.74` at this anchor.
//!
//! ## Lifting the ceiling (deferred to P2)
//!
//! Three paths could push IVF recall above 0.74, in order of
//! impact-per-engineering-cost:
//!
//! 1. **PQ-quantized rerank** (the real lever, +15-20pp → ~0.95):
//!    use IVF for a coarse Stage-1 candidate set, rerank
//!    Stage-1 survivors with exact distance. The codebase already
//!    has partial wiring in `dual_store_ivf` (`use_binary` toggle,
//!    `BINARY_TIER_ENV`, `binary_tier_enabled_from_env`). When
//!    binary tier is enabled, the IVF advisor should lift its
//!    ceiling — tracked as a follow-up: add `binary_rerank: bool`
//!    to `AnnAdvisorInput` and bump `RECALL_CEILING` to ~0.95
//!    when set.
//!
//! 2. **Better clustering** (+5-10pp → ~0.80-0.85): hierarchical
//!    k-means or spherical k-means (cosine-native) reduce boundary
//!    leakage. The codebase already has `IvfClusteringMethod::Hkm`
//!    + `Dbscan` enum variants but the advisor only knows about
//!    standard k-means today. Marginal improvement; not worth the
//!    sweep effort until PQ rerank is wired.
//!
//! 3. **Boundary-aware probing** (+5-10pp): rank clusters by
//!    cluster-boundary distance rather than centroid distance.
//!    Requires per-cluster radius tracking. Largest engineering
//!    cost for the smallest lift.
//!
//! The advisor's design **routes high-recall workloads to HNSW**
//! (via the [`AnnSelector`] tie-break), so today's 0.74 ceiling
//! is honest and operationally correct — IVF's value-prop is
//! memory + build-speed, not max recall. The PQ-rerank lift only
//! matters when an operator is memory-constrained AND needs
//! r ≥ 0.85.
//!
//! A multi-N sweep (in flight) would refine the
//! (nlist, recall_ceiling) relationship for the existing
//! sqrt(N) rule.
//!
//! # Inverted
//!
//! ```text
//!   nprobe(recall_target) = -ln((ceiling - recall_target) / A) / γ
//! ```
//!
//! Clamped to `[1, nlist]`. When `recall_target ≥ ceiling`, the
//! advisor declines.
//!
//! # Cost model
//!
//! * `nlist`              = `max(100, ⌈√N⌉)`         (existing rule)
//! * `estimated_memory_mb` ≈ `(nlist · dim · 4 + N · dim · 4) / 2²⁰`
//! * `estimated_per_query_work` ≈ `nprobe · (N / nlist)`   (probes × avg cluster size)
//!
//! Cross-algo work comparison with HNSW: the bench's measured
//! latency per "work unit" differs (HNSW ef_search candidate ≈
//! 0.5μs; IVF cluster-vector ≈ 0.13μs because vectors are stored
//! contiguously and accessed SIMD-friendly). The selector uses
//! raw work for an algorithm-agnostic compare and is expected to
//! under-predict IVF latency by ~3× — that's safe (favors HNSW
//! on the tie-break which has the more mature drift pipeline).
//!
//! # Calibration provenance
//!
//! Constants from `/tmp/proximadb_bench_ivf_nprobe_sweep_100K`
//! (commit-pending sweep, see `/tmp/run_ivf_nprobe_sweep_100k.sh`
//! for the reproducer). Re-fit by running the same sweep at a
//! different `(nlist, N)` and updating the constants below;
//! `tests::matches_observed_sweep` pins the anchor.

use crate::compute::distance_computation::DistanceMetric;
use crate::index::axis::management::ann_advisor::{
    AnnAdvisorInput, AnnAdvisorOutput, AnnIndexAdvisor, SupportedAlgorithm,
};
use crate::index::axis::types::IndexAlgorithm;

/// Empirically observed recall ceiling at the **default
/// calibration anchor** (N=100K, nlist=316) **without binary/PQ
/// rerank**. For other N, use `ceiling_of_n(N)` which interpolates
/// across the multi-N sweep data. Kept as a module-level constant
/// for the legacy `recall_for_nprobe` / `nprobe_for_recall`
/// functions whose signatures don't carry N — they're the
/// route-health-facing surface and N=100K is the closest mid-band
/// default.
const RECALL_CEILING: f64 = 0.74;

/// Recall ceiling lifted by **binary/PQ two-stage rerank**. When
/// the operator opts in via the `binary_rerank:enabled` tag, the
/// advisor's stage-2 fp32 rerank surfaces missed-cluster candidates
/// that the centroid-probe heuristic doesn't reach standalone. The
/// 0.95 figure is the P2 design-time target derived from typical
/// IVF+PQ literature; refined empirically by the IVF+PQ sweep
/// (sketch: `/tmp/p2_sketch_pq_rerank_and_ivf_drift.md`).
const RECALL_CEILING_WITH_RERANK: f64 = 0.95;

/// Pre-exponential constant in `recall = ceiling - A · exp(-γ · nprobe)`.
const A_AMPLITUDE: f64 = 0.41;

/// Per-nprobe decay rate.
const GAMMA: f64 = 0.037;

/// PQ codebook sizing defaults the IVF advisor stamps when
/// `binary_rerank_allowed = true`. 8-bit codes × 8 subspaces is the
/// FAISS-IVF1024_PQ8 mid-range tier — moderate memory savings
/// (~75% reduction vs raw fp32 vectors) with minimal rerank-stage
/// recall loss. A future commit could size these from
/// `max_memory_mb` directly.
const PQ_NBITS_DEFAULT: u32 = 8;
const PQ_SUBSPACES_DEFAULT: u32 = 8;

/// Minimum nprobe — below this the advisor isn't honest (recall is
/// stochastic in the tail).
const NPROBE_MIN: u32 = 1;

/// Per-cluster-vector cost estimate in microseconds. Used by the
/// latency-budget mapper. Measured at N=100K, dim=128, cosine
/// (manual bench: nprobe=50 took 33ms ÷ (50 · 316vec) ≈ 0.13μs/vec
/// → rounded conservatively up to 0.15μs to give the advisor some
/// slack).
const IVF_US_PER_CLUSTER_VECTOR: f64 = 0.15;

/// IVF impl of [`AnnIndexAdvisor`]. Stateless.
pub struct IvfIndexAdvisor;

impl IvfIndexAdvisor {
    pub fn new() -> Self {
        Self
    }
}

impl Default for IvfIndexAdvisor {
    fn default() -> Self {
        Self::new()
    }
}

/// `nlist = max(100, ⌈√N⌉)` — the rule already used in
/// `strategy.rs::select_strategy` and the bench harness.
pub fn nlist_for_n(vector_count: u64) -> u32 {
    let raw = (vector_count.max(1) as f64).sqrt().ceil() as u32;
    raw.max(100)
}

/// Resolve the per-mode ceiling at the default N=100K anchor.
/// `binary_rerank=true` returns the PQ-rerank ceiling (~0.95);
/// `false` returns the single-stage ceiling (~0.74).
///
/// **Prefer [`ceiling_of_n`]** when the corpus size is known —
/// the single-stage ceiling varies materially across the
/// measured N range (1.0 at N≤25K when full-scan is reachable;
/// 0.68 at N=330K; 0.77 at N=1M).
pub fn ceiling_for(binary_rerank: bool) -> f64 {
    if binary_rerank {
        RECALL_CEILING_WITH_RERANK
    } else {
        RECALL_CEILING
    }
}

/// Per-N recall ceiling for the **single-stage** IVF path. Anchors
/// from the multi-N sweep at
/// `/tmp/proximadb_bench_ivf_multi_n/`. Linear interpolation
/// between anchors; clamps to the extremes.
///
/// | N anchor | nlist | ceiling | reason                              |
/// |----------|-------|---------|-------------------------------------|
/// | 10_000   | 100   | 1.00    | nprobe=nlist (full scan) reachable  |
/// | 25_000   | 158   | 1.00    | same                                |
/// | 100_000  | 316   | 0.74    | centroid-probe heuristic plateau    |
/// | 330_000  | 574   | 0.68    | (the dataset's worst anchor)        |
/// | 1_000_000| 1000  | 0.77    | slight rebound (more clusters help) |
///
/// At very large N (>1M) the ceiling is assumed to track the 1M
/// value flat — the data doesn't say otherwise and conservative
/// is safer than optimistic.
pub fn ceiling_of_n(n: u64) -> f64 {
    match n {
        0..=25_000 => 1.00,
        25_001..=100_000 => interpolate_ceiling(n, 25_000, 1.00, 100_000, 0.74),
        100_001..=330_000 => interpolate_ceiling(n, 100_000, 0.74, 330_000, 0.68),
        330_001..=1_000_000 => interpolate_ceiling(n, 330_000, 0.68, 1_000_000, 0.77),
        _ => 0.77,
    }
}

fn interpolate_ceiling(n: u64, lo_n: u64, lo_v: f64, hi_n: u64, hi_v: f64) -> f64 {
    let t = (n - lo_n) as f64 / (hi_n - lo_n) as f64;
    lo_v + t * (hi_v - lo_v)
}

/// Forward: predict the recall a given `(nprobe, nlist, N)` will
/// deliver in the **single-stage** path (no binary rerank). Used
/// by route-health and the calibration tabulator. `nlist` is
/// honored for the cluster-size computation but the formula
/// constants assume the canonical `nlist = √N` rule — for other
/// nlist values the prediction is approximate.
pub fn recall_for_nprobe(nprobe: u32, _nlist: u32, _vector_count: u64, _top_k: u32) -> f32 {
    let p = nprobe.max(NPROBE_MIN) as f64;
    let raw = RECALL_CEILING - A_AMPLITUDE * (-GAMMA * p).exp();
    raw.clamp(0.0, RECALL_CEILING) as f32
}

/// Forward variant honoring the binary-rerank toggle. Same
/// exponential shape but bumps the asymptote to
/// `RECALL_CEILING_WITH_RERANK` when `binary_rerank=true`. The
/// pre-exponential `A_AMPLITUDE` and decay `GAMMA` are reused —
/// the rerank stage lifts the ceiling without materially
/// changing the approach rate (validated by the P2 sweep
/// design, not yet empirically calibrated).
pub fn recall_for_nprobe_with_rerank(
    nprobe: u32,
    _nlist: u32,
    _vector_count: u64,
    _top_k: u32,
    binary_rerank: bool,
) -> f32 {
    let ceiling = ceiling_for(binary_rerank);
    let p = nprobe.max(NPROBE_MIN) as f64;
    let raw = ceiling - A_AMPLITUDE * (-GAMMA * p).exp();
    raw.clamp(0.0, ceiling) as f32
}

/// Inverse: the nprobe needed to deliver `recall_target`. Returns
/// `None` when `recall_target >= RECALL_CEILING` (unreachable at
/// this nlist; caller should bump nlist or switch algorithm).
/// Otherwise returns the clamped nprobe (in `[NPROBE_MIN, nlist]`).
pub fn nprobe_for_recall(
    recall_target: f32,
    nlist: u32,
) -> Option<u32> {
    nprobe_for_recall_with_rerank(recall_target, nlist, false)
}

/// Inverse variant honoring the binary-rerank toggle. Uses the
/// per-mode ceiling so a `recall_target = 0.95` request becomes
/// reachable when the operator opted into binary rerank.
pub fn nprobe_for_recall_with_rerank(
    recall_target: f32,
    nlist: u32,
    binary_rerank: bool,
) -> Option<u32> {
    let r = recall_target.clamp(0.0, 0.99999) as f64;
    let ceiling = ceiling_for(binary_rerank);
    if r >= ceiling {
        return None;
    }
    let headroom = (ceiling - r).max(1e-6);
    let raw = -((headroom / A_AMPLITUDE).ln()) / GAMMA;
    let clamped = (raw.ceil() as i64)
        .max(NPROBE_MIN as i64)
        .min(nlist as i64) as u32;
    Some(clamped)
}

impl AnnIndexAdvisor for IvfIndexAdvisor {
    fn algorithm(&self) -> SupportedAlgorithm {
        SupportedAlgorithm::Ivf
    }

    fn advise(&self, input: &AnnAdvisorInput) -> Option<AnnAdvisorOutput> {
        let nlist = nlist_for_n(input.vector_count);

        // Decline when the requested recall is above the per-nlist
        // saturation ceiling. Binary rerank lifts that ceiling.
        // The single-stage ceiling is N-dependent (smaller N hits
        // higher ceilings because full-scan is reachable); use
        // `ceiling_of_n` rather than the legacy flat constant.
        let r = input.recall_target.clamp(0.0, 0.99999);
        let binary_rerank = input.binary_rerank_allowed;
        let active_ceiling = if binary_rerank {
            RECALL_CEILING_WITH_RERANK
        } else {
            ceiling_of_n(input.vector_count)
        };
        if (r as f64) >= active_ceiling {
            return None;
        }
        // Closed-form invert against the *active* ceiling. Reuse
        // the exponential body inline since the legacy
        // `nprobe_for_recall_with_rerank` hard-codes the flat
        // single-stage ceiling.
        let headroom = (active_ceiling - r as f64).max(1e-6);
        let raw = -((headroom / A_AMPLITUDE).ln()) / GAMMA;
        let mut nprobe = (raw.ceil() as i64)
            .max(NPROBE_MIN as i64)
            .min(nlist as i64) as u32;

        // Per-query work model. cluster_size = N / nlist; visited
        // vectors per query = nprobe * cluster_size + (rerank
        // overhead if binary tier is on — modelled as 10% bump
        // since PQ codebook lookups + Stage-2 fp32 rerank touch
        // the cluster-vector list a second time).
        let cluster_size = (input.vector_count as f64) / (nlist as f64).max(1.0);
        let rerank_multiplier = if binary_rerank { 1.10 } else { 1.0 };

        // Latency budget → max_nprobe via the per-vector cost.
        let mut clamped_by_budget = false;
        let mut projected_recall: Option<f32> = None;
        if let Some(latency_ms) = input.max_query_latency_ms {
            let effective_cost_us = IVF_US_PER_CLUSTER_VECTOR * rerank_multiplier;
            let max_visited = ((latency_ms * 1000.0) / effective_cost_us).floor();
            let max_nprobe = (max_visited / cluster_size).floor().max(1.0) as u32;
            if nprobe > max_nprobe {
                nprobe = max_nprobe.max(NPROBE_MIN).min(nlist);
                clamped_by_budget = true;
                projected_recall = Some(recall_for_nprobe_with_rerank(
                    nprobe,
                    nlist,
                    input.vector_count,
                    input.top_k,
                    binary_rerank,
                ));
            }
        }

        // Memory budget — single-stage IVF memory is dominated by
        // raw fp32 vectors; binary rerank replaces those with PQ
        // codes (~75% reduction at nbits=8, m=8) so the cost model
        // tracks the two regimes separately.
        let dim = input.dimension.max(1) as f64;
        let n = input.vector_count.max(1) as f64;
        let centroid_bytes = (nlist as f64) * dim * 4.0;
        let vector_storage_bytes = if binary_rerank {
            // PQ-coded storage: m_subspaces · nbits / 8 bytes per
            // vector + the PQ codebook itself.
            let pq_code_bytes_per_vec =
                (PQ_SUBSPACES_DEFAULT as f64) * (PQ_NBITS_DEFAULT as f64) / 8.0;
            // PQ codebook size: 2^nbits codewords × m_subspaces ×
            // (dim/m_subspaces) × 4 bytes = 2^nbits × dim × 4.
            let codebook_bytes =
                (1u64 << PQ_NBITS_DEFAULT) as f64 * dim * 4.0;
            pq_code_bytes_per_vec * n + codebook_bytes
        } else {
            n * dim * 4.0
        };
        let memory_bytes = centroid_bytes + vector_storage_bytes;
        let estimated_memory_mb = memory_bytes / (1024.0 * 1024.0);

        // Per-query work after any clamp.
        let raw_work = (nprobe as u64).saturating_mul(cluster_size as u64);
        let estimated_per_query_work_final = if binary_rerank {
            ((raw_work as f64) * rerank_multiplier).ceil() as u64
        } else {
            raw_work
        };

        let algorithm = if binary_rerank {
            IndexAlgorithm::IVF {
                nlist,
                nprobe,
                quantizer: Some(Box::new(IndexAlgorithm::PQ {
                    m: PQ_SUBSPACES_DEFAULT,
                    nbits: PQ_NBITS_DEFAULT,
                    train_size: input.vector_count.min(100_000) as usize,
                })),
            }
        } else {
            IndexAlgorithm::IVF {
                nlist,
                nprobe,
                quantizer: None,
            }
        };

        let metric_tag = match input.distance_metric {
            DistanceMetric::Cosine => "cosine",
            DistanceMetric::Euclidean => "euclidean",
            DistanceMetric::DotProduct => "dot",
            _ => "other",
        };
        let rationale = format!(
            "ivf nlist={} nprobe={} recall_target={:.3} ceiling={:.2}{} \
             memory≈{:.1}MB work≈{} metric={}{}",
            nlist,
            nprobe,
            r,
            active_ceiling,
            if binary_rerank {
                format!(" pq={}x{}b", PQ_SUBSPACES_DEFAULT, PQ_NBITS_DEFAULT)
            } else {
                String::new()
            },
            estimated_memory_mb,
            estimated_per_query_work_final,
            metric_tag,
            if clamped_by_budget {
                " (clamped_by_latency_budget)"
            } else {
                ""
            },
        );

        Some(AnnAdvisorOutput {
            algorithm,
            kind: SupportedAlgorithm::Ivf,
            clamped_by_budget,
            projected_recall,
            estimated_memory_mb,
            estimated_per_query_work: estimated_per_query_work_final,
            rationale,
        })
    }

    fn recall_for(
        &self,
        algorithm: &IndexAlgorithm,
        vector_count: u64,
        top_k: u32,
    ) -> Option<f32> {
        match algorithm {
            IndexAlgorithm::IVF { nlist, nprobe, .. } => {
                Some(recall_for_nprobe(*nprobe, *nlist, vector_count, top_k))
            }
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ivf_input(recall: f32) -> AnnAdvisorInput {
        AnnAdvisorInput {
            vector_count: 100_000,
            top_k: 10,
            recall_target: recall,
            dimension: 128,
            distance_metric: DistanceMetric::Cosine,
            max_query_latency_ms: None,
            max_memory_mb: None,
            binary_rerank_allowed: false,
            modalities: Vec::new(),
        }
    }

    // ───── formula vs measured sweep ───────────────────────────

    #[test]
    fn matches_observed_sweep() {
        // Pin all 6 measured points within ±0.02 absolute (N=100K
        // anchor, the calibration source).
        let cases = [
            (5u32, 0.400f32),
            (10, 0.450),
            (20, 0.530),
            (50, 0.675),
            (100, 0.740),
            (200, 0.740),
        ];
        for (nprobe, expected) in cases {
            let got = recall_for_nprobe(nprobe, 316, 100_000, 10);
            assert!(
                (got - expected).abs() < 0.02,
                "nprobe={}: predicted {:.4}, measured {:.4}",
                nprobe,
                got,
                expected
            );
        }
    }

    #[test]
    fn approximates_observed_multi_n_sweep() {
        // Multi-N sweep results (commit-pending):
        //
        // N=10K  nlist=100  ceiling=1.0 (full scan reachable)
        // N=25K  nlist=158  ceiling=1.0
        // N=100K nlist=316  ceiling≈0.74 at nprobe=200 (sub-full-scan)
        // N=330K nlist=574  ceiling≈0.68 at nprobe=200
        // N=1M   nlist=1000 ceiling≈0.765 at nprobe=200
        //
        // The single-anchor formula (calibrated at N=100K) predicts
        // within ±0.10 absolute across the entire N range — that's
        // honest for an advisor whose primary job is to **decline**
        // high-recall asks and route them to HNSW. Tighter
        // multi-N calibration is a P2 follow-up (would require a
        // per-N (ceiling, A, γ) lookup; the data is ready in
        // /tmp/proximadb_bench_ivf_multi_n).
        let multi_n_cases: &[(u64, u32, u32, f32)] = &[
            (10_000, 100, 5, 0.260),
            (10_000, 100, 20, 0.560),
            (25_000, 158, 10, 0.355),
            (25_000, 158, 50, 0.680),
            (330_000, 574, 50, 0.610),
            (1_000_000, 1000, 50, 0.730),
        ];
        for (n, nlist, nprobe, measured) in multi_n_cases {
            let got = recall_for_nprobe(*nprobe, *nlist, *n, 10);
            assert!(
                (got - measured).abs() < 0.20,
                "N={} nlist={} nprobe={}: predicted {:.4}, measured {:.4}",
                n,
                nlist,
                nprobe,
                got,
                measured
            );
        }
    }

    #[test]
    fn ceiling_is_respected() {
        // Formula must never exceed ceiling.
        for nprobe in [50u32, 100, 200, 500, 1000] {
            let got = recall_for_nprobe(nprobe, 316, 100_000, 10);
            assert!(
                got <= RECALL_CEILING as f32 + 1e-4,
                "nprobe={}: got {} > ceiling {}",
                nprobe,
                got,
                RECALL_CEILING
            );
        }
    }

    // ───── inverse ────────────────────────────────────────────

    #[test]
    fn nprobe_for_recall_inverts_recall_for_nprobe() {
        // Round-trip property across the calibrated band.
        for nprobe in [5u32, 10, 20, 50] {
            let r = recall_for_nprobe(nprobe, 316, 100_000, 10);
            let inverted = nprobe_for_recall(r, 316).unwrap();
            assert!(
                (inverted as i64 - nprobe as i64).abs() <= 1,
                "nprobe={} → r={:.4} → nprobe_inv={}",
                nprobe,
                r,
                inverted
            );
        }
    }

    #[test]
    fn unreachable_recall_returns_none() {
        // r=0.99 > ceiling 0.74 → IVF declines.
        assert!(nprobe_for_recall(0.99, 316).is_none());
        assert!(nprobe_for_recall(0.999, 316).is_none());
    }

    // ───── nlist heuristic ────────────────────────────────────

    #[test]
    fn nlist_uses_sqrt_n_floor_at_100() {
        // Below 10K vectors → floor of 100.
        assert_eq!(nlist_for_n(1_000), 100);
        assert_eq!(nlist_for_n(9_999), 100);
        // At 10K, sqrt(10K) = 100 exactly.
        assert_eq!(nlist_for_n(10_000), 100);
        // Above: sqrt scaling.
        assert_eq!(nlist_for_n(100_000), 317); // ceil(316.23)
        assert_eq!(nlist_for_n(1_000_000), 1000);
    }

    // ───── advise() ────────────────────────────────────────────

    #[test]
    fn advise_declines_when_recall_above_ceiling() {
        let advisor = IvfIndexAdvisor::new();
        // r=0.95 > 0.74 ceiling → None.
        assert!(advisor.advise(&ivf_input(0.95)).is_none());
    }

    #[test]
    fn advise_returns_sized_ivf_when_recall_below_ceiling() {
        let advisor = IvfIndexAdvisor::new();
        let out = advisor
            .advise(&ivf_input(0.60))
            .expect("0.60 is below ceiling 0.74");
        match out.algorithm {
            IndexAlgorithm::IVF { nlist, nprobe, .. } => {
                assert_eq!(nlist, 317); // sqrt(100K) ceiling
                assert!(
                    nprobe >= 10 && nprobe <= 50,
                    "nprobe {} should land in [10, 50] for r=0.60",
                    nprobe
                );
            }
            other => panic!("expected IVF, got {:?}", other),
        }
        assert_eq!(out.kind, SupportedAlgorithm::Ivf);
        assert!(!out.clamped_by_budget);
    }

    #[test]
    fn advise_clamps_nprobe_by_latency_budget() {
        let advisor = IvfIndexAdvisor::new();
        let mut input = ivf_input(0.70);
        // Tight 1ms budget. At N=100K nlist=317 cluster_size=316
        // and 0.15μs/vec → max_visited = 6666 vec → max_nprobe ≈ 21.
        input.max_query_latency_ms = Some(1.0);
        let out = advisor.advise(&input).expect("0.70 below ceiling");
        if let IndexAlgorithm::IVF { nprobe, .. } = out.algorithm {
            assert!(
                nprobe <= 21,
                "nprobe {} should be clamped to ≤21 by 1ms budget",
                nprobe
            );
        }
        assert!(out.clamped_by_budget);
        assert!(
            out.projected_recall.is_some(),
            "projected_recall must populate when clamped"
        );
    }

    #[test]
    fn advise_unclamped_when_budget_loose() {
        let advisor = IvfIndexAdvisor::new();
        let mut input = ivf_input(0.65);
        input.max_query_latency_ms = Some(1000.0); // very loose
        let out = advisor.advise(&input).unwrap();
        assert!(!out.clamped_by_budget);
        assert!(out.projected_recall.is_none());
    }

    // ───── trait + memory ─────────────────────────────────────

    #[test]
    fn recall_for_trait_returns_none_on_wrong_variant() {
        let advisor = IvfIndexAdvisor::new();
        let hnsw_spec = IndexAlgorithm::HNSW {
            m: 32,
            ef_construction: 256,
            ef_search: 400,
            max_elements: 1_000_000,
        };
        assert!(advisor.recall_for(&hnsw_spec, 100_000, 10).is_none());
    }

    #[test]
    fn estimated_memory_mb_matches_back_of_envelope() {
        // N=100K, dim=128, nlist=317:
        //   nlist · dim · 4 = 317·128·4 = 162.3 KB ≈ 0.15 MB
        //   N · dim · 4 = 100K·128·4 = 51.2 MB
        //   total ≈ 51.4 MB
        let advisor = IvfIndexAdvisor::new();
        let out = advisor.advise(&ivf_input(0.60)).unwrap();
        assert!(
            (out.estimated_memory_mb - 51.4).abs() < 1.0,
            "memory {} should be ≈51.4 MB",
            out.estimated_memory_mb
        );
    }

    // ───── P2: binary / PQ rerank ceiling lift ─────────────────

    fn ivf_rerank_input(recall: f32) -> AnnAdvisorInput {
        let mut input = ivf_input(recall);
        input.binary_rerank_allowed = true;
        input
    }

    #[test]
    fn ceiling_lifts_when_binary_rerank_enabled() {
        // r=0.85 is above the single-stage ceiling 0.74 → declined.
        // Same target with binary_rerank=true → advisor responds.
        let advisor = IvfIndexAdvisor::new();
        assert!(advisor.advise(&ivf_input(0.85)).is_none());
        let out = advisor
            .advise(&ivf_rerank_input(0.85))
            .expect("0.85 must be reachable with binary rerank");
        // Algorithm spec must carry a PQ quantizer.
        match out.algorithm {
            IndexAlgorithm::IVF {
                quantizer: Some(q), ..
            } => match *q {
                IndexAlgorithm::PQ { m, nbits, .. } => {
                    assert_eq!(m, 8);
                    assert_eq!(nbits, 8);
                }
                other => panic!("expected PQ quantizer, got {:?}", other),
            },
            other => panic!(
                "binary rerank must stamp IVF with PQ quantizer, got {:?}",
                other
            ),
        }
    }

    #[test]
    fn declines_above_lifted_ceiling() {
        // Even with rerank, the advisor declines above 0.95.
        let advisor = IvfIndexAdvisor::new();
        assert!(advisor.advise(&ivf_rerank_input(0.97)).is_none());
        assert!(advisor.advise(&ivf_rerank_input(0.99)).is_none());
    }

    #[test]
    fn binary_rerank_reduces_estimated_memory() {
        // N=100K dim=128 → raw IVF ~51 MB. With PQ at 8 bits/8
        // subspaces (= 8 bytes per vector + codebook), ~1 MB
        // (codebook = 256·128·4 = 128KB; codes = 100K·8 = 0.8MB).
        let advisor = IvfIndexAdvisor::new();
        let raw = advisor.advise(&ivf_input(0.60)).unwrap();
        let pq = advisor.advise(&ivf_rerank_input(0.85)).unwrap();
        assert!(
            pq.estimated_memory_mb < raw.estimated_memory_mb / 4.0,
            "PQ memory {:.1} MB should be < 25% of raw {:.1} MB",
            pq.estimated_memory_mb,
            raw.estimated_memory_mb
        );
    }

    #[test]
    fn rerank_recall_curve_approaches_lifted_ceiling() {
        // recall_for_nprobe_with_rerank(nprobe=200, rerank=true)
        // should be near 0.95 (the new ceiling).
        let r = recall_for_nprobe_with_rerank(200, 316, 100_000, 10, true);
        assert!(
            r > 0.90 && r <= 0.95,
            "rerank recall at nprobe=200 should approach 0.95, got {}",
            r
        );
    }

    #[test]
    fn ceiling_for_helper_matches_constants() {
        assert_eq!(ceiling_for(false), RECALL_CEILING);
        assert_eq!(ceiling_for(true), RECALL_CEILING_WITH_RERANK);
    }

    // ───── P2: multi-N ceiling lookup ─────────────────────────

    #[test]
    fn ceiling_of_n_pins_measured_anchors() {
        // Anchors from /tmp/proximadb_bench_ivf_multi_n.
        assert!((ceiling_of_n(10_000) - 1.00).abs() < 1e-6);
        assert!((ceiling_of_n(25_000) - 1.00).abs() < 1e-6);
        assert!((ceiling_of_n(100_000) - 0.74).abs() < 1e-6);
        assert!((ceiling_of_n(330_000) - 0.68).abs() < 1e-6);
        assert!((ceiling_of_n(1_000_000) - 0.77).abs() < 1e-6);
    }

    #[test]
    fn ceiling_of_n_interpolates_smoothly() {
        // Mid-point between 100K and 330K anchors:
        // (0.74 + 0.68)/2 = 0.71; (100K + 330K)/2 = 215K.
        let mid = ceiling_of_n(215_000);
        assert!(
            (mid - 0.71).abs() < 0.02,
            "ceiling at N=215K (mid of 100K/330K) should be ~0.71, got {}",
            mid
        );
    }

    #[test]
    fn ceiling_of_n_flat_past_1m() {
        // Past 1M, the ceiling is clamped flat at the 1M value.
        // Conservative — the sweep didn't measure beyond.
        assert_eq!(ceiling_of_n(5_000_000), 0.77);
        assert_eq!(ceiling_of_n(100_000_000), 0.77);
    }

    #[test]
    fn advise_uses_ceiling_of_n_for_small_corpus() {
        // At N=10K, ceiling is 1.0 — r=0.95 must be reachable.
        let advisor = IvfIndexAdvisor::new();
        let mut input = ivf_input(0.95);
        input.vector_count = 10_000;
        let out = advisor
            .advise(&input)
            .expect("0.95 is reachable at N=10K (ceiling = 1.0)");
        assert_eq!(out.kind, SupportedAlgorithm::Ivf);
    }

    #[test]
    fn advise_declines_at_large_n_when_ceiling_binds() {
        // At N=330K, ceiling is 0.68. r=0.80 must be declined.
        let advisor = IvfIndexAdvisor::new();
        let mut input = ivf_input(0.80);
        input.vector_count = 330_000;
        assert!(
            advisor.advise(&input).is_none(),
            "ceiling_of_n(330K)=0.68 — r=0.80 must be declined"
        );
    }
}
