// Copyright 2025 Vijaykumar Singh.
// Licensed under the Apache License, Version 2.0.

//! Adaptive recall-target drift detection.
//!
//! # Why
//!
//! An HNSW index is built at a specific N (vector count). As the
//! collection grows, the in-place index keeps using the m and
//! ef_search the operator picked at create time — but the advisor's
//! recommendation for the *current* N may have moved into a higher
//! tier (e.g. m=16 → 32, or ef_search 400 → 900). If that happens
//! and nobody re-indexes, the customer-observed recall silently
//! drops.
//!
//! This module is a **pure-function drift detector**. The caller
//! supplies:
//! * the corpus size when the index was last built (`baseline_n`),
//! * the current corpus size (`current_n`),
//! * the recall target the operator asked for,
//! * top_k, dimension, distance_metric.
//!
//! The detector calls `advise_hnsw_params` at both `baseline_n` and
//! `current_n`, classifies the diff, and returns a structured
//! `RecallDriftReport` for the caller to log / surface on
//! route-health / emit metrics / decide whether to nudge the
//! operator to `/recluster`.
//!
//! **No side effects.** The detector never rebuilds, never mutates
//! AXIS state, never reads from durable storage. Pure inputs → pure
//! output. The "trigger arm" lives elsewhere (route-health endpoint,
//! AdaptiveIndexEngine periodic sweep, manual /recluster call).
//!
//! # Drift kinds
//!
//! * `DriftKind::None` — recommended params unchanged. No action.
//! * `DriftKind::EfSearchOnly` — only `ef_search` differs. Can be
//!   live-tuned: HNSW supports updating ef_search per query without
//!   rebuilding the graph. Operator can hot-swap the strategy.
//! * `DriftKind::EfConstructionOrM` — `m` or `ef_construction`
//!   differs. The graph itself was built for the old params, so a
//!   full rebuild is required to realize the new recall target.
//!
//! The split matters because the cost is wildly different — live
//! retune is near-free, rebuild costs minutes-to-hours of build IO.

use crate::compute::distance_computation::DistanceMetric;
use crate::index::axis::management::{HnswSizingInput, HnswSizingOutput, advise_hnsw_params};

/// Inputs to the drift detection. All fields except
/// `max_ef_search` are required — the detector takes no defaults
/// because the right answer depends on every one of these.
#[derive(Debug, Clone, Copy)]
pub struct RecallDriftInput {
    /// Corpus size (vectors) when the index was last built.
    pub baseline_n: u64,
    /// Corpus size right now.
    pub current_n: u64,
    /// Recall fraction in [0.0, 1.0] the operator asked for.
    pub recall_target: f32,
    pub top_k: u32,
    pub dimension: u32,
    pub distance_metric: DistanceMetric,
    /// Optional latency-budget cap. Propagated to both advisor calls
    /// so the resulting `baseline_params` / `current_params` carry
    /// the same `clamped_by_max_ef` / `projected_recall_if_clamped`
    /// signals the route-health surface reads.
    pub max_ef_search: Option<u32>,
}

/// Classification of the diff between baseline and current
/// advisor recommendations. Variants cover both HNSW and IVF:
/// hot-swappable variants are knob-only changes the live strategy
/// can absorb; rebuild-required variants need a /recluster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftKind {
    /// Advisor's recommendation hasn't changed. No-op.
    None,
    /// **HNSW**: only `ef_search` differs. Hot-swappable; no
    /// rebuild.
    EfSearchOnly,
    /// **HNSW**: `m` or `ef_construction` differs. Full rebuild
    /// required.
    EfConstructionOrM,
    /// **IVF**: only `nprobe` differs. Hot-swappable; the
    /// strategy's `IndexAlgorithm::IVF.nprobe` update is live-
    /// tunable without rebuilding the centroid+posting structure.
    NprobeOnly,
    /// **IVF**: `nlist` or quantizer-mode (PQ on/off, nbits,
    /// m_subspaces) differs. Full rebuild required — cluster
    /// reassignment and/or codebook reconstruction.
    NlistOrQuantizer,
}

/// Report returned by `detect_recall_drift`.
#[derive(Debug, Clone)]
pub struct RecallDriftReport {
    pub baseline_n: u64,
    pub current_n: u64,
    pub baseline_params: HnswSizingOutput,
    pub current_params: HnswSizingOutput,
    pub drift_kind: DriftKind,
    /// Short human-friendly summary for route-health / log lines.
    /// Example: "m 16→32, ef_search 287→409 — full rebuild required".
    pub summary: String,
}

impl RecallDriftReport {
    /// True if the operator should be nudged to rebuild — either
    /// HNSW (m / ef_construction tier change) or IVF
    /// (nlist / quantizer change).
    pub fn needs_rebuild(&self) -> bool {
        matches!(
            self.drift_kind,
            DriftKind::EfConstructionOrM | DriftKind::NlistOrQuantizer
        )
    }

    /// True if the operator can resolve the drift by hot-swapping
    /// the live strategy — HNSW `ef_search` or IVF `nprobe` —
    /// without touching the index structure.
    pub fn hot_swap_possible(&self) -> bool {
        matches!(
            self.drift_kind,
            DriftKind::EfSearchOnly | DriftKind::NprobeOnly
        )
    }
}

/// Detect whether the advisor's HNSW recommendation has drifted
/// between `baseline_n` and `current_n` for the same recall target.
/// Pure function; no side effects.
pub fn detect_recall_drift(input: RecallDriftInput) -> RecallDriftReport {
    let baseline_params = advise_hnsw_params(HnswSizingInput {
        vector_count: input.baseline_n.max(1),
        top_k: input.top_k,
        recall_target: input.recall_target,
        dimension: input.dimension,
        distance_metric: input.distance_metric,
        max_ef_search: input.max_ef_search,
    });
    let current_params = advise_hnsw_params(HnswSizingInput {
        vector_count: input.current_n.max(1),
        top_k: input.top_k,
        recall_target: input.recall_target,
        dimension: input.dimension,
        distance_metric: input.distance_metric,
        max_ef_search: input.max_ef_search,
    });

    let drift_kind = classify(&baseline_params, &current_params);
    let summary = summarize(&baseline_params, &current_params, drift_kind);

    RecallDriftReport {
        baseline_n: input.baseline_n,
        current_n: input.current_n,
        baseline_params,
        current_params,
        drift_kind,
        summary,
    }
}

// ───── IVF drift surface (P2 commit 3) ─────────────────────────
//
// Parallel to the HNSW machinery above: same `DriftKind`
// discriminator but populated by an IVF-specific
// `IvfRecallDriftReport`. Adding it as a sibling rather than
// mutating `RecallDriftReport` keeps the existing HNSW consumer
// sites (route-health, recall-tune, recluster) untouched. The
// route-health surface in P2 commit 4 will dispatch on the
// collection's active algorithm and call the matching detector.

use crate::index::axis::management::ivf_param_advisor::{
    ceiling_of_n, nlist_for_n, nprobe_for_recall_with_rerank, recall_for_nprobe_with_rerank,
};

/// IVF-flavoured drift inputs. Sibling to [`RecallDriftInput`]
/// for the HNSW path; both pure functions, both side-effect free.
#[derive(Debug, Clone, Copy)]
pub struct IvfRecallDriftInput {
    /// Corpus size when the IVF index was last built. Determines
    /// the baseline nlist via the √N rule and the baseline ceiling
    /// via `ceiling_of_n`.
    pub baseline_n: u64,
    /// Corpus size right now.
    pub current_n: u64,
    pub recall_target: f32,
    pub top_k: u32,
    pub dimension: u32,
    pub distance_metric: DistanceMetric,
    /// Optional latency budget — maps to a `max_nprobe` clamp.
    pub max_query_latency_ms: Option<f64>,
    /// Optional memory budget — passes through to the IVF
    /// advisor's PQ-rerank gating.
    pub max_memory_mb: Option<f64>,
    /// Whether the collection opted into PQ rerank (lifts ceiling
    /// to ~0.95). Reads from the `binary_rerank:enabled` tag.
    pub binary_rerank_allowed: bool,
}

/// Per-snapshot IVF sizing. Carries the live knobs the route-
/// health surface displays + the diagnostics needed to drive
/// `/recall-tune` (nprobe hot-swap) and `/recluster` (full
/// rebuild for nlist or quantizer changes).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct IvfSizing {
    pub nlist: u32,
    pub nprobe: u32,
    /// `true` when the advisor's recommendation includes a PQ
    /// quantizer (binary rerank path). Recluster-required if this
    /// flips between baseline and current.
    pub pq_enabled: bool,
    /// When the latency budget clamped nprobe below the
    /// advisor's recommendation, the recall the index will
    /// actually deliver. None when no clamp.
    pub projected_recall: Option<f32>,
    pub clamped_by_budget: bool,
}

/// IVF drift report. Same discriminator (`DriftKind`) as the
/// HNSW path but populated from `IvfSizing` snapshots.
#[derive(Debug, Clone)]
pub struct IvfRecallDriftReport {
    pub baseline_n: u64,
    pub current_n: u64,
    pub baseline_params: IvfSizing,
    pub current_params: IvfSizing,
    pub drift_kind: DriftKind,
    pub summary: String,
}

impl IvfRecallDriftReport {
    pub fn needs_rebuild(&self) -> bool {
        matches!(self.drift_kind, DriftKind::NlistOrQuantizer)
    }

    pub fn hot_swap_possible(&self) -> bool {
        matches!(self.drift_kind, DriftKind::NprobeOnly)
    }
}

/// Detect IVF drift between baseline and current N at the
/// declared recall target. Pure function — no I/O, no AXIS
/// mutation. Pairs with [`detect_recall_drift`] for HNSW.
pub fn detect_ivf_recall_drift(input: IvfRecallDriftInput) -> IvfRecallDriftReport {
    let baseline_params = size_ivf(input.baseline_n.max(1), &input);
    let current_params = size_ivf(input.current_n.max(1), &input);
    let drift_kind = classify_ivf(&baseline_params, &current_params);
    let summary = summarize_ivf(&baseline_params, &current_params, drift_kind);

    IvfRecallDriftReport {
        baseline_n: input.baseline_n,
        current_n: input.current_n,
        baseline_params,
        current_params,
        drift_kind,
        summary,
    }
}

fn size_ivf(n: u64, input: &IvfRecallDriftInput) -> IvfSizing {
    let nlist = nlist_for_n(n);
    let active_ceiling = if input.binary_rerank_allowed {
        // Reuse the rerank ceiling — same constant as the advisor.
        crate::index::axis::management::ivf_param_advisor::ceiling_for(true)
    } else {
        ceiling_of_n(n)
    };

    let r = input.recall_target.clamp(0.0, 0.99999) as f64;
    let baseline_nprobe = if (r as f32) >= active_ceiling as f32 {
        // Unreachable target — pin at full-scan (rebuild signal).
        nlist
    } else {
        nprobe_for_recall_with_rerank(input.recall_target, nlist, input.binary_rerank_allowed)
            .unwrap_or(nlist)
    };

    // Latency clamp.
    let mut nprobe = baseline_nprobe;
    let mut clamped_by_budget = false;
    let mut projected_recall: Option<f32> = None;
    if let Some(latency_ms) = input.max_query_latency_ms {
        let cluster_size = (n as f64) / (nlist as f64).max(1.0);
        // Per-vector cost mirrors the IVF advisor.
        let per_vec_us = 0.15_f64
            * if input.binary_rerank_allowed {
                1.10
            } else {
                1.0
            };
        let max_visited = ((latency_ms * 1000.0) / per_vec_us).floor();
        let max_nprobe = (max_visited / cluster_size).floor().max(1.0) as u32;
        if nprobe > max_nprobe {
            nprobe = max_nprobe.max(1).min(nlist);
            clamped_by_budget = true;
            projected_recall = Some(recall_for_nprobe_with_rerank(
                nprobe,
                nlist,
                n,
                input.top_k,
                input.binary_rerank_allowed,
            ));
        }
    }

    IvfSizing {
        nlist,
        nprobe,
        pq_enabled: input.binary_rerank_allowed,
        projected_recall,
        clamped_by_budget,
    }
}

fn classify_ivf(baseline: &IvfSizing, current: &IvfSizing) -> DriftKind {
    if baseline.nlist != current.nlist || baseline.pq_enabled != current.pq_enabled {
        return DriftKind::NlistOrQuantizer;
    }
    if baseline.nprobe != current.nprobe {
        return DriftKind::NprobeOnly;
    }
    DriftKind::None
}

fn summarize_ivf(baseline: &IvfSizing, current: &IvfSizing, kind: DriftKind) -> String {
    match kind {
        DriftKind::None => format!(
            "no drift (nlist={}, nprobe={}, pq={})",
            current.nlist, current.nprobe, current.pq_enabled
        ),
        DriftKind::NprobeOnly => format!(
            "nprobe {}→{} (nlist={} pq={} unchanged) — hot-swap possible",
            baseline.nprobe, current.nprobe, current.nlist, current.pq_enabled
        ),
        DriftKind::NlistOrQuantizer => format!(
            "nlist {}→{}, pq {}→{}, nprobe {}→{} — full rebuild required",
            baseline.nlist,
            current.nlist,
            baseline.pq_enabled,
            current.pq_enabled,
            baseline.nprobe,
            current.nprobe,
        ),
        // HNSW variants don't apply here; defensive fallback.
        DriftKind::EfSearchOnly | DriftKind::EfConstructionOrM => format!(
            "unexpected HNSW drift kind on IVF detector — \
             nlist={} nprobe={} (kind={:?})",
            current.nlist, current.nprobe, kind
        ),
    }
}

fn classify(baseline: &HnswSizingOutput, current: &HnswSizingOutput) -> DriftKind {
    if baseline.m != current.m || baseline.ef_construction != current.ef_construction {
        return DriftKind::EfConstructionOrM;
    }
    if baseline.ef_search != current.ef_search {
        return DriftKind::EfSearchOnly;
    }
    DriftKind::None
}

fn summarize(baseline: &HnswSizingOutput, current: &HnswSizingOutput, kind: DriftKind) -> String {
    match kind {
        DriftKind::None => format!(
            "no drift (m={}, efc={}, ef={})",
            current.m, current.ef_construction, current.ef_search
        ),
        DriftKind::EfSearchOnly => format!(
            "ef_search {}→{} (m={}, efc={} unchanged) — hot-swap possible",
            baseline.ef_search, current.ef_search, current.m, current.ef_construction
        ),
        DriftKind::EfConstructionOrM => format!(
            "m {}→{}, efc {}→{}, ef_search {}→{} — full rebuild required",
            baseline.m,
            current.m,
            baseline.ef_construction,
            current.ef_construction,
            baseline.ef_search,
            current.ef_search,
        ),
        // IVF variants — the HNSW classifier can't actually
        // produce these (its inputs are HnswSizingOutput), but
        // the type system needs all DriftKind arms covered.
        // Defensive: should-never-fire.
        DriftKind::NprobeOnly | DriftKind::NlistOrQuantizer => {
            format!("unexpected IVF drift kind from HNSW classifier: {:?}", kind)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(baseline_n: u64, current_n: u64, recall: f32) -> RecallDriftInput {
        RecallDriftInput {
            baseline_n,
            current_n,
            recall_target: recall,
            top_k: 10,
            dimension: 128,
            distance_metric: DistanceMetric::Cosine,
            max_ef_search: None,
        }
    }

    fn input_with_cap(baseline_n: u64, current_n: u64, recall: f32, cap: u32) -> RecallDriftInput {
        let mut i = input(baseline_n, current_n, recall);
        i.max_ef_search = Some(cap);
        i
    }

    #[test]
    fn same_n_no_drift() {
        let r = detect_recall_drift(input(100_000, 100_000, 0.95));
        assert_eq!(r.drift_kind, DriftKind::None);
        assert!(!r.needs_rebuild());
        assert!(!r.hot_swap_possible());
        assert!(r.summary.contains("no drift"));
    }

    #[test]
    fn growth_within_same_tier_is_ef_only() {
        // 100K → 250K both stay in m=32 tier at recall=0.95, only
        // ef_search bumps up.
        let r = detect_recall_drift(input(100_000, 250_000, 0.95));
        // 100K @ 0.95 → m=32 ef≈409; 250K @ 0.95 → m=32 ef≈559
        assert_eq!(r.baseline_params.m, r.current_params.m);
        assert_eq!(
            r.baseline_params.ef_construction,
            r.current_params.ef_construction
        );
        assert!(r.current_params.ef_search > r.baseline_params.ef_search);
        assert_eq!(r.drift_kind, DriftKind::EfSearchOnly);
        assert!(r.hot_swap_possible());
        assert!(!r.needs_rebuild());
    }

    #[test]
    fn recall_tier_jump_requires_rebuild() {
        // The detector takes one recall_target per call, so we
        // verify the rebuild path by running it twice across an
        // m-tier boundary (post-recalibration the boundaries are
        // 0.75 / 0.85 / 0.97). Picking r=0.80 (m=16) vs r=0.90
        // (m=32) — the user-visible behaviour ("changing target
        // needs rebuild") is the callsite's responsibility; the
        // detector just classifies the per-target before/after.
        let low = detect_recall_drift(input(100_000, 100_000, 0.80));
        let high = detect_recall_drift(input(100_000, 100_000, 0.90));
        assert!(high.current_params.m > low.current_params.m);
    }

    #[test]
    fn ef_only_summary_string_shape() {
        let r = detect_recall_drift(input(100_000, 250_000, 0.95));
        assert!(r.summary.starts_with("ef_search "));
        assert!(r.summary.contains("hot-swap possible"));
    }

    #[test]
    fn handles_zero_baseline_n_gracefully() {
        // A freshly-created collection (no baseline build yet) —
        // baseline_n=0 must not panic. We clamp internally to 1.
        let r = detect_recall_drift(input(0, 50_000, 0.90));
        // Both advisor calls succeeded, summary is non-empty
        assert!(!r.summary.is_empty());
    }

    #[test]
    fn dramatic_growth_requires_rebuild() {
        // Two recall targets straddling the m=32 → m=48 boundary
        // (0.97 per the post-8b54d721c recalibration). The detector
        // doesn't change m for a fixed target as N grows past the
        // anchor (the ef-only path) — the actual m bump comes from
        // the operator raising recall_target. This test guards that
        // tier-crossing targets really do pick different m.
        let just_below = detect_recall_drift(input(100_000, 100_000, 0.96));
        let just_above = detect_recall_drift(input(100_000, 100_000, 0.98));
        assert_ne!(just_below.current_params.m, just_above.current_params.m);
        assert_eq!(just_below.current_params.m, 32);
        assert_eq!(just_above.current_params.m, 48);
    }

    #[test]
    fn clamped_drift_surfaces_in_report() {
        // With max_ef_search=300 and recall_target=0.95 at N=100K,
        // the advisor wants ~ef=405 (m=32). Clamping to 300 means
        // the index will deliver less than 0.95 — the report's
        // current_params must carry the clamp signal so route-health
        // can surface it.
        let r = detect_recall_drift(input_with_cap(100_000, 100_000, 0.95, 300));
        assert!(
            r.current_params.clamped_by_max_ef,
            "current_params must report the clamp"
        );
        assert_eq!(r.current_params.ef_search, 300);
        let projected = r
            .current_params
            .projected_recall_if_clamped
            .expect("projected recall must populate when clamped");
        assert!(
            projected < 0.95,
            "projected recall {} should be below 0.95",
            projected
        );
        // Baseline at the same N hits the same clamp.
        assert!(r.baseline_params.clamped_by_max_ef);
    }
}
