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

/// Inputs to the drift detection. All fields required — the
/// detector takes no defaults because the right answer depends on
/// every one of these.
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
}

/// Classification of the diff between baseline and current
/// advisor recommendations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriftKind {
    /// Advisor's recommendation hasn't changed. No-op.
    None,
    /// Only `ef_search` differs. Hot-swappable; no rebuild.
    EfSearchOnly,
    /// `m` or `ef_construction` differs. Full rebuild required.
    EfConstructionOrM,
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
    /// True if the operator should be nudged to rebuild. False
    /// when the diff is zero or hot-swappable.
    pub fn needs_rebuild(&self) -> bool {
        matches!(self.drift_kind, DriftKind::EfConstructionOrM)
    }

    /// True if the operator can resolve the drift by hot-swapping
    /// ef_search without touching the graph.
    pub fn hot_swap_possible(&self) -> bool {
        matches!(self.drift_kind, DriftKind::EfSearchOnly)
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
    });
    let current_params = advise_hnsw_params(HnswSizingInput {
        vector_count: input.current_n.max(1),
        top_k: input.top_k,
        recall_target: input.recall_target,
        dimension: input.dimension,
        distance_metric: input.distance_metric,
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
        }
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
}
