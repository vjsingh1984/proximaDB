//! Cost-routed fusion planning (TD-141 / F-F of `docs/12-design/CROSS_MODAL_FUSION_SEAM_2026_06_22.adoc`).
//!
//! Decides, from **measured** per-query quantities (each source's weight + its actual candidate count),
//! how to route a fused query (D9):
//!
//! * **fuse only when genuinely multi-modal** — a single surviving source is a passthrough, not a fusion;
//! * **drop a negligible-weight modality** — BoomHQ: a source whose weight is tiny relative to the
//!   strongest only adds noise (and round-trips) to the blend;
//! * **budget each surviving modality** — split a total candidate budget proportional to weight, so a
//!   heavier (or more selective) modality gets a larger candidate set (BoomHQ's per-modality `kᵢ`).
//!
//! This is a **threshold router over measured quantities, not a learned model** ("measure, don't
//! assert"; UNIFY/BoomHQ). It is intentionally inert by default (no drops, unbounded budget) so callers
//! opt in. Wiring this into the live `FusionService` seed/expand budgets and the `ComputeScheduler`
//! route cost is the remaining TD-141 integration; this module is the policy core.

use crate::core::search::cross_modal_fusion::{SourceCandidates, SourceId};

/// Routing policy knobs (all measured-trace driven; inert by default).
#[derive(Debug, Clone, Copy)]
pub struct RoutePolicy {
    /// Drop a source whose weight is below this fraction of the strongest source's weight (BoomHQ:
    /// a negligible-weight modality only adds noise). `0.0` disables the drop.
    pub min_weight_fraction: f32,
    /// Total candidate budget split (proportional to weight) across surviving sources. `usize::MAX`
    /// disables budgeting (each source keeps its full pool).
    pub total_budget: usize,
}

impl Default for RoutePolicy {
    fn default() -> Self {
        Self {
            min_weight_fraction: 0.0,
            total_budget: usize::MAX,
        }
    }
}

/// The routing decision for one fused query.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FusionRoute {
    /// Surviving sources and their allocated candidate budget `kᵢ`, by `SourceId`.
    pub budgets: Vec<(SourceId, usize)>,
    /// `true` iff ≥2 sources survive — fusion engages; `false` → single-modal passthrough.
    pub fuse: bool,
    /// How many input sources were dropped (empty or negligible-weight).
    pub dropped: usize,
}

/// Plan the route from measured source shapes (each source's weight + actual candidate count).
pub fn plan_route(sources: &[SourceCandidates], policy: &RoutePolicy) -> FusionRoute {
    let max_weight = sources
        .iter()
        .map(|source| source.weight)
        .fold(0.0_f32, f32::max);

    // A source survives if it contributed candidates AND its weight is not negligible vs the strongest.
    let surviving: Vec<&SourceCandidates> = sources
        .iter()
        .filter(|source| {
            !source.scores.is_empty()
                && (policy.min_weight_fraction <= 0.0
                    || max_weight <= 0.0
                    || source.weight >= policy.min_weight_fraction * max_weight)
        })
        .collect();

    let total_weight: f32 = surviving.iter().map(|source| source.weight).sum();
    let budgets = surviving
        .iter()
        .map(|source| {
            let budget = if policy.total_budget == usize::MAX || total_weight <= 0.0 {
                policy.total_budget
            } else {
                // kᵢ = round(total · wᵢ/Σw), at least 1 so a surviving source is never starved.
                ((policy.total_budget as f32 * source.weight / total_weight).round() as usize)
                    .max(1)
            };
            (source.source.clone(), budget)
        })
        .collect::<Vec<_>>();

    FusionRoute {
        dropped: sources.len() - surviving.len(),
        fuse: surviving.len() >= 2,
        budgets,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// A source with `weight` and `n` dummy candidates.
    fn cand(source: SourceId, weight: f32, n: usize) -> SourceCandidates {
        let scores: HashMap<String, f32> = (0..n).map(|i| (format!("oid{i}"), 1.0)).collect();
        SourceCandidates::new(source, weight, scores)
    }

    #[test]
    fn single_modal_does_not_fuse() {
        let route = plan_route(&[cand(SourceId::Vector, 1.0, 5)], &RoutePolicy::default());
        assert!(!route.fuse, "one source → passthrough, not fusion");
        assert_eq!(route.budgets.len(), 1);
    }

    #[test]
    fn two_sources_engage_fusion() {
        let sources = [
            cand(SourceId::Vector, 1.0, 5),
            cand(SourceId::Graph, 1.0, 3),
        ];
        let route = plan_route(&sources, &RoutePolicy::default());
        assert!(route.fuse);
        assert_eq!(route.dropped, 0);
    }

    #[test]
    fn empty_source_is_dropped() {
        let sources = [
            cand(SourceId::Vector, 1.0, 5),
            cand(SourceId::Graph, 1.0, 0),
        ];
        let route = plan_route(&sources, &RoutePolicy::default());
        assert_eq!(route.dropped, 1, "the empty source contributes nothing");
        assert!(!route.fuse, "only one source left → no fusion");
    }

    #[test]
    fn negligible_weight_modality_is_dropped() {
        // Graph weight 0.01 is below 10% of the max (1.0) → dropped (BoomHQ).
        let sources = [
            cand(SourceId::Vector, 1.0, 5),
            cand(SourceId::Graph, 0.01, 5),
        ];
        let policy = RoutePolicy {
            min_weight_fraction: 0.1,
            ..RoutePolicy::default()
        };
        let route = plan_route(&sources, &policy);
        assert_eq!(route.dropped, 1);
        assert!(!route.fuse);
        assert_eq!(route.budgets[0].0, SourceId::Vector);
    }

    #[test]
    fn budget_splits_proportional_to_weight() {
        // weights 3:1, total budget 8 → ~6 and ~2.
        let sources = [
            cand(SourceId::Vector, 3.0, 100),
            cand(SourceId::Graph, 1.0, 100),
        ];
        let policy = RoutePolicy {
            total_budget: 8,
            ..RoutePolicy::default()
        };
        let route = plan_route(&sources, &policy);
        let budget_of = |s: &SourceId| {
            route
                .budgets
                .iter()
                .find(|(id, _)| id == s)
                .map(|(_, b)| *b)
        };
        assert_eq!(budget_of(&SourceId::Vector), Some(6));
        assert_eq!(budget_of(&SourceId::Graph), Some(2));
    }

    #[test]
    fn default_policy_is_inert() {
        // No drops, unbounded budget, fuse iff ≥2 — a safe no-op router.
        let sources = [
            cand(SourceId::Vector, 1.0, 5),
            cand(SourceId::Document, 0.001, 5),
        ];
        let route = plan_route(&sources, &RoutePolicy::default());
        assert_eq!(route.dropped, 0, "inert default drops nothing");
        assert!(route.fuse);
        assert!(route.budgets.iter().all(|(_, b)| *b == usize::MAX));
    }
}
