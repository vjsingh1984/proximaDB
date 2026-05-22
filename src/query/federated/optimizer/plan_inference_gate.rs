// Plan-inference gate — decides whether to honor a v2 model recommendation
// or fall back to the v1 deterministic planner.
//
// `plan_v2_inference::InferenceArtifactRegistry` resolves whichever
// inferencer the tenant has registered for their `(tenant, collection,
// model)` scope; on miss it falls through to the v1 linear fallback.
// The gate's job is the *next* step: given a v2 recommendation, decide
// whether to actually use it. The decision considers:
//
//   1. Confidence threshold — v2 inferencers emit a confidence in
//      [0, 1]; below the configured floor we ignore them.
//   2. Pending-artifact source — when the registry returned an
//      `ArtifactPlanInferencer` whose model hasn't loaded yet (source
//      `"uae-artifact-pending"`), we treat the recommendation as v1
//      and never log it as a v2 deviation.
//   3. Safety overrides — v2 may not route to QuantizedGraphThenExact
//      for a tenant whose RecallProbeGate is closed, regardless of
//      confidence. This is the LLD §5 recall-safety guarantee carried
//      through the v2 path.
//   4. v1 / v2 agreement — when the two emit the same plan, the gate
//      passes the v2 plan through (cheap) and records the agreement
//      so the trace shows v2 vs v1 stayed aligned.
//
// The gate emits a typed `GateOutcome` carrying the plan and a
// bounded static-string source label so the trace + audit can record
// *why* the chosen plan won.

use crate::observability::search_plan_trace::{FilterStrategy, IndexRoute};
use crate::query::federated::optimizer::plan_v2_inference::PlanInference;

/// Tunable thresholds.
#[derive(Debug, Clone, Copy)]
pub struct InferenceGateConfig {
    /// Minimum v2 confidence required to honor a deviation from v1.
    /// `0.0` honors any non-pending v2 output; `1.0` never honors v2.
    pub confidence_threshold: f64,
}

impl Default for InferenceGateConfig {
    fn default() -> Self {
        Self { confidence_threshold: 0.7 }
    }
}

/// Output of the gate. `Honor` carries the plan we'll actually execute
/// plus a bounded source string; `Fallback` carries the v1 plan and a
/// reason so the trace can record why v2 lost.
#[derive(Debug, Clone, PartialEq)]
pub enum GateOutcome {
    /// Use the recommended plan.
    Honor {
        filter_strategy: FilterStrategy,
        index_route: IndexRoute,
        confidence: f64,
        source: &'static str,
    },
    /// Use v1 instead.
    Fallback {
        filter_strategy: FilterStrategy,
        index_route: IndexRoute,
        reason: &'static str,
    },
}

impl GateOutcome {
    pub fn filter_strategy(&self) -> &FilterStrategy {
        match self {
            GateOutcome::Honor { filter_strategy, .. } => filter_strategy,
            GateOutcome::Fallback { filter_strategy, .. } => filter_strategy,
        }
    }
    pub fn index_route(&self) -> &IndexRoute {
        match self {
            GateOutcome::Honor { index_route, .. } => index_route,
            GateOutcome::Fallback { index_route, .. } => index_route,
        }
    }
    pub fn is_honor(&self) -> bool {
        matches!(self, GateOutcome::Honor { .. })
    }
}

/// Bounded source labels — pinned to a closed set so observability +
/// the trace can filter on them without translation.
pub mod source {
    pub const V2_HONORED: &str = "v2-honored";
    pub const V2_AGREED_V1: &str = "v2-agreed-v1";
}

/// Bounded fallback-reason labels.
pub mod reason {
    pub const PENDING_ARTIFACT: &str = "pending_artifact";
    pub const CONFIDENCE_BELOW_THRESHOLD: &str = "confidence_below_threshold";
    pub const QUANTIZED_ROUTE_BLOCKED: &str = "quantized_route_blocked";
}

/// Inputs the gate consumes per request.
#[derive(Debug, Clone)]
pub struct GateInputs<'a> {
    /// The v2 inferencer's output.
    pub v2: &'a PlanInference,
    /// The v1 fallback inferencer's output. The gate uses this both as
    /// the literal fallback plan and as the "agreed?" comparison anchor.
    pub v1: &'a PlanInference,
    /// `true` when the tenant's RecallProbeGate is open for the
    /// targeted collection. Used to veto quantized-route v2
    /// recommendations on collections whose probe set hasn't passed.
    pub recall_probe_open: bool,
}

/// Run the gate. Pure given the inputs + config.
pub fn decide(inputs: &GateInputs<'_>, config: &InferenceGateConfig) -> GateOutcome {
    // Helper: build the fallback variant with the v1 plan and the
    // given reason. Functional-record-update syntax doesn't work on
    // enum variants in stable Rust, so we spell out the fields.
    let fallback = |reason: &'static str| GateOutcome::Fallback {
        filter_strategy: inputs.v1.filter_strategy.clone(),
        index_route: inputs.v1.index_route.clone(),
        reason,
    };

    // Step 1: pending-artifact passthrough. When the registry returned
    // an artifact wrapper whose model hasn't loaded, the v2.source is
    // "uae-artifact-pending" and the recommendation is just v1 with a
    // different label. Pass v1 through and don't log it as a deviation.
    if inputs.v2.source == "uae-artifact-pending" {
        return fallback(reason::PENDING_ARTIFACT);
    }

    // Step 2: confidence floor.
    let confidence = inputs.v2.confidence.clamp(0.0, 1.0);
    if confidence < config.confidence_threshold {
        return fallback(reason::CONFIDENCE_BELOW_THRESHOLD);
    }

    // Step 3: quantized-route safety veto. If v2 recommends the
    // quantized route but the tenant's recall probe gate is closed for
    // the collection, deny. This is the LLD §5 invariant ported through
    // the v2 path.
    if matches!(inputs.v2.index_route, IndexRoute::QuantizedGraphThenExact)
        && !inputs.recall_probe_open
    {
        return fallback(reason::QUANTIZED_ROUTE_BLOCKED);
    }

    // Step 4: v2 wins. Distinguish "v2 honored because it differed from
    // v1" from "v2 honored and happened to agree with v1" — both honor
    // v2 but observability cares about the alignment.
    let agrees_with_v1 = inputs.v2.filter_strategy == inputs.v1.filter_strategy
        && inputs.v2.index_route == inputs.v1.index_route;
    let source = if agrees_with_v1 {
        source::V2_AGREED_V1
    } else {
        source::V2_HONORED
    };
    GateOutcome::Honor {
        filter_strategy: inputs.v2.filter_strategy.clone(),
        index_route: inputs.v2.index_route.clone(),
        confidence,
        source,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pi(s: FilterStrategy, r: IndexRoute, c: f64, src: &'static str) -> PlanInference {
        PlanInference {
            filter_strategy: s,
            index_route: r,
            confidence: c,
            source: src,
        }
    }

    fn v1() -> PlanInference {
        pi(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.5,
            "linear-v1-fallback",
        )
    }

    fn cfg(threshold: f64) -> InferenceGateConfig {
        InferenceGateConfig { confidence_threshold: threshold }
    }

    #[test]
    fn pending_artifact_passes_v1_through_with_distinct_reason() {
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::QuantizedGraphThenExact,
            0.99,
            "uae-artifact-pending",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: false },
            &cfg(0.7),
        );
        match out {
            GateOutcome::Fallback { reason, filter_strategy, index_route } => {
                assert_eq!(reason, reason::PENDING_ARTIFACT);
                assert_eq!(filter_strategy, FilterStrategy::HybridFilter);
                assert_eq!(index_route, IndexRoute::FullPrecisionGraph);
            }
            other => panic!("expected pending_artifact fallback, got {other:?}"),
        }
    }

    #[test]
    fn confidence_below_threshold_falls_back_to_v1() {
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::FullPrecisionGraph,
            0.4,
            "uae-v1",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: true },
            &cfg(0.7),
        );
        match out {
            GateOutcome::Fallback { reason, .. } => {
                assert_eq!(reason, reason::CONFIDENCE_BELOW_THRESHOLD);
            }
            other => panic!("expected confidence fallback, got {other:?}"),
        }
    }

    #[test]
    fn confidence_at_threshold_honors_v2() {
        // Boundary: confidence == threshold honors v2. Strict less-than
        // is the cutoff; equality passes.
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::FullPrecisionGraph,
            0.7,
            "uae-v1",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: true },
            &cfg(0.7),
        );
        assert!(out.is_honor());
    }

    #[test]
    fn quantized_route_blocked_when_recall_probe_closed() {
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::QuantizedGraphThenExact,
            0.95,
            "uae-v1",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: false },
            &cfg(0.5),
        );
        match out {
            GateOutcome::Fallback { reason, .. } => {
                assert_eq!(reason, reason::QUANTIZED_ROUTE_BLOCKED);
            }
            other => panic!("expected quantized_route_blocked fallback, got {other:?}"),
        }
    }

    #[test]
    fn quantized_route_honored_when_recall_probe_open() {
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::QuantizedGraphThenExact,
            0.95,
            "uae-v1",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: true },
            &cfg(0.5),
        );
        match out {
            GateOutcome::Honor { source, index_route, .. } => {
                assert_eq!(source, source::V2_HONORED);
                assert_eq!(index_route, IndexRoute::QuantizedGraphThenExact);
            }
            other => panic!("expected honor, got {other:?}"),
        }
    }

    #[test]
    fn agreement_with_v1_uses_distinct_source_label() {
        let v2 = pi(
            FilterStrategy::HybridFilter,
            IndexRoute::FullPrecisionGraph,
            0.95,
            "uae-v1",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: false },
            &cfg(0.5),
        );
        match out {
            GateOutcome::Honor { source, .. } => {
                assert_eq!(source, source::V2_AGREED_V1);
            }
            other => panic!("expected honor, got {other:?}"),
        }
    }

    #[test]
    fn divergence_from_v1_uses_v2_honored_label() {
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::FullPrecisionGraph,
            0.95,
            "uae-v1",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: false },
            &cfg(0.5),
        );
        match out {
            GateOutcome::Honor { source, .. } => {
                assert_eq!(source, source::V2_HONORED);
            }
            other => panic!("expected v2-honored, got {other:?}"),
        }
    }

    #[test]
    fn confidence_clamps_out_of_band_values() {
        // Misbehaving inferencer emits confidence > 1.0. The gate must
        // honor (clamp doesn't push below threshold) but the recorded
        // confidence on the outcome is clamped.
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::FullPrecisionGraph,
            5.0,
            "uae-v1",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: true },
            &cfg(0.5),
        );
        match out {
            GateOutcome::Honor { confidence, .. } => {
                assert_eq!(confidence, 1.0);
            }
            other => panic!("expected honor, got {other:?}"),
        }
    }

    #[test]
    fn negative_confidence_falls_back() {
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::FullPrecisionGraph,
            -0.5,
            "uae-v1",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: true },
            &cfg(0.5),
        );
        // Clamped to 0.0, which is below the 0.5 threshold.
        match out {
            GateOutcome::Fallback { reason, .. } => {
                assert_eq!(reason, reason::CONFIDENCE_BELOW_THRESHOLD);
            }
            other => panic!("expected confidence fallback, got {other:?}"),
        }
    }

    #[test]
    fn pending_artifact_short_circuits_before_confidence_check() {
        // Even with very high confidence, a pending artifact never
        // deviates from v1 — the label semantics matter more than the
        // numeric confidence on a fallback wrapper.
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::QuantizedGraphThenExact,
            1.0,
            "uae-artifact-pending",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: true },
            &cfg(0.5),
        );
        match out {
            GateOutcome::Fallback { reason, .. } => {
                assert_eq!(reason, reason::PENDING_ARTIFACT);
            }
            other => panic!("expected pending_artifact, got {other:?}"),
        }
    }

    #[test]
    fn pending_artifact_short_circuits_before_quantized_check() {
        // Pending source wins over quantized-route safety: both would
        // fall back, but pending is the more informative reason because
        // it tells the operator "we don't even have a model yet" vs
        // "the model wanted quantized but the probe is closed".
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::QuantizedGraphThenExact,
            0.95,
            "uae-artifact-pending",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: false },
            &cfg(0.5),
        );
        match out {
            GateOutcome::Fallback { reason, .. } => {
                assert_eq!(reason, reason::PENDING_ARTIFACT);
            }
            other => panic!("expected pending_artifact, got {other:?}"),
        }
    }

    #[test]
    fn outcome_accessors_return_chosen_plan() {
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::FullPrecisionGraph,
            0.95,
            "uae-v1",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: true },
            &cfg(0.5),
        );
        assert_eq!(out.filter_strategy(), &FilterStrategy::PreFilter);
        assert_eq!(out.index_route(), &IndexRoute::FullPrecisionGraph);
        assert!(out.is_honor());
    }

    #[test]
    fn fallback_outcome_returns_v1_plan() {
        let v2 = pi(
            FilterStrategy::PreFilter,
            IndexRoute::QuantizedGraphThenExact,
            0.1,
            "uae-v1",
        );
        let v1 = v1();
        let out = decide(
            &GateInputs { v2: &v2, v1: &v1, recall_probe_open: true },
            &cfg(0.5),
        );
        // Fallback emits the v1 plan, not v2's.
        assert_eq!(out.filter_strategy(), &v1.filter_strategy);
        assert_eq!(out.index_route(), &v1.index_route);
        assert!(!out.is_honor());
    }

    #[test]
    fn source_and_reason_labels_are_bounded_static_strings() {
        // Pin the closed set so observability can register fixed
        // Prometheus labels without re-derivation.
        assert_eq!(source::V2_HONORED, "v2-honored");
        assert_eq!(source::V2_AGREED_V1, "v2-agreed-v1");
        assert_eq!(reason::PENDING_ARTIFACT, "pending_artifact");
        assert_eq!(reason::CONFIDENCE_BELOW_THRESHOLD, "confidence_below_threshold");
        assert_eq!(reason::QUANTIZED_ROUTE_BLOCKED, "quantized_route_blocked");
    }
}
