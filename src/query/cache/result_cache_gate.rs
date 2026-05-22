// Result cache decision gate.
//
// Bundles two Phase 2 primitives the result cache always consults
// together:
//
//   1. `PerCategoryPolicy` — per-workload-category similarity threshold,
//      TTL, and quota. Code queries cluster densely so they tolerate 0.92;
//      conversational queries are sparse so they need 0.78 to ever hit.
//
//   2. `MismatchCostLearner` (CUCB-SC) — per-region online learner that
//      decides whether the *mismatch cost* of serving a near-but-not-
//      identical result exceeds the configured allowed_cost.
//
// The runtime currently has to call both and stitch them together. This
// gate provides a single `decide()` call that returns a typed decision
// + a structured reason string so observability + the trace can record
// *why* the cache served vs rejected.

use std::time::Duration;

use crate::query::cache::mismatch_cost::{
    MismatchCostLearner, MismatchDecision, Region as MismatchRegion,
};
use crate::query::cache::per_category_policy::{CategoryPolicy, PerCategoryPolicy};

/// Input to the gate: caller supplies the runtime-side inputs (similarity,
/// age, current values) and the gate looks up the per-category policy +
/// runs the mismatch learner.
#[derive(Debug, Clone)]
pub struct GateInputs<'a> {
    /// Workload category the gateway classified the request into.
    pub category: &'a str,
    /// Tenant id — passed through into the mismatch region key.
    pub tenant_id: &'a str,
    /// Cosine similarity between the incoming query and the cached entry.
    pub similarity: f64,
    /// Age of the cached entry — compared against the category TTL.
    pub age: Duration,
}

/// Decision output. Each variant carries a static-string reason so
/// observability has something stable to filter on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Serve from the cache.
    Serve {
        reason: &'static str,
        /// The per-category policy that admitted the entry.
        category_label: &'static str,
    },
    /// Refuse — re-execute the query.
    Reject { reason: &'static str },
}

impl GateDecision {
    pub fn is_serve(&self) -> bool {
        matches!(self, GateDecision::Serve { .. })
    }
}

/// Result cache gate. Holds owned policy + learner references — the
/// runtime constructs one of these per cache instance and reuses it.
pub struct ResultCacheGate<'a> {
    pub policy: &'a PerCategoryPolicy,
    pub mismatch: &'a MismatchCostLearner,
}

impl<'a> ResultCacheGate<'a> {
    pub fn new(policy: &'a PerCategoryPolicy, mismatch: &'a MismatchCostLearner) -> Self {
        Self { policy, mismatch }
    }

    /// Run the full decision pipeline.
    ///
    /// Order:
    ///   1. Resolve the per-category policy (unknown categories fall back
    ///      to the safe default).
    ///   2. Reject when the cached entry has aged past its TTL.
    ///   3. Reject when the similarity is below the category threshold.
    ///   4. Defer to the mismatch learner — its decision is the final word.
    pub async fn decide(&self, inputs: &GateInputs<'_>) -> GateDecision {
        let policy: CategoryPolicy = self.policy.lookup(inputs.category);

        if inputs.age > policy.ttl {
            return GateDecision::Reject {
                reason: "ttl_expired",
            };
        }
        if inputs.similarity < policy.similarity_threshold {
            return GateDecision::Reject {
                reason: "below_similarity_threshold",
            };
        }
        // Defer to the mismatch learner. Its similarity floor may be
        // stricter than the category threshold; we let it decide.
        let region = MismatchRegion::new(inputs.tenant_id, inputs.category);
        match self.mismatch.decide(&region, inputs.similarity).await {
            MismatchDecision::Accept { .. } => GateDecision::Serve {
                reason: "category_threshold_and_mismatch_ok",
                category_label: policy.prom_label,
            },
            MismatchDecision::Reject { reason, .. } => GateDecision::Reject {
                reason: classify_reject(reason),
            },
        }
    }
}

fn classify_reject(reason: &'static str) -> &'static str {
    // Pass through with a stable label for observability — the mismatch
    // learner emits "below_similarity" | "cold_region" | "above_allowed_cost",
    // which the audit dashboards already filter on.
    reason
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query::cache::mismatch_cost::MismatchConfig;
    use std::collections::HashMap;
    use std::time::Duration;

    fn policy_with_threshold(threshold: f64, ttl: Duration) -> PerCategoryPolicy {
        let mut table: HashMap<String, CategoryPolicy> = HashMap::new();
        table.insert(
            "code".to_string(),
            CategoryPolicy {
                similarity_threshold: threshold,
                ttl,
                quota: 1_000,
                prom_label: "code",
            },
        );
        PerCategoryPolicy::from_table(table)
    }

    fn warm_learner_for_low_cost() -> MismatchCostLearner {
        let learner = MismatchCostLearner::new(MismatchConfig {
            similarity_floor: 0.5,
            allowed_cost: 0.5,
            decay_seconds: 3600,
        });
        learner
    }

    #[tokio::test]
    async fn ttl_expired_rejects_before_similarity_check() {
        let policy = policy_with_threshold(0.99, Duration::from_secs(60));
        let learner = warm_learner_for_low_cost();
        let gate = ResultCacheGate::new(&policy, &learner);
        let d = gate
            .decide(&GateInputs {
                category: "code",
                tenant_id: "t",
                similarity: 0.99,
                age: Duration::from_secs(120),
            })
            .await;
        match d {
            GateDecision::Reject { reason } => assert_eq!(reason, "ttl_expired"),
            other => panic!("expected ttl_expired reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn similarity_below_threshold_rejects() {
        let policy = policy_with_threshold(0.92, Duration::from_secs(60));
        let learner = warm_learner_for_low_cost();
        let gate = ResultCacheGate::new(&policy, &learner);
        let d = gate
            .decide(&GateInputs {
                category: "code",
                tenant_id: "t",
                similarity: 0.85,
                age: Duration::from_secs(1),
            })
            .await;
        match d {
            GateDecision::Reject { reason } => {
                assert_eq!(reason, "below_similarity_threshold");
            }
            other => panic!("expected below_similarity_threshold reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn cold_mismatch_region_rejects_first_hit() {
        // Even with similarity + TTL ok, a cold mismatch region rejects.
        let policy = policy_with_threshold(0.5, Duration::from_secs(60));
        let learner = warm_learner_for_low_cost();
        let gate = ResultCacheGate::new(&policy, &learner);
        let d = gate
            .decide(&GateInputs {
                category: "code",
                tenant_id: "t",
                similarity: 0.95,
                age: Duration::from_secs(1),
            })
            .await;
        // Cold region — mismatch learner rejects with reason "cold_region".
        match d {
            GateDecision::Reject { reason } => assert_eq!(reason, "cold_region"),
            other => panic!("expected cold_region reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn warm_low_cost_region_serves() {
        let policy = policy_with_threshold(0.5, Duration::from_secs(60));
        let learner = warm_learner_for_low_cost();
        // Warm up the learner — 50 observations of low cost.
        for _ in 0..50 {
            learner
                .observe(MismatchRegion::new("t", "code"), 0.05)
                .await;
        }
        let gate = ResultCacheGate::new(&policy, &learner);
        let d = gate
            .decide(&GateInputs {
                category: "code",
                tenant_id: "t",
                similarity: 0.95,
                age: Duration::from_secs(1),
            })
            .await;
        match d {
            GateDecision::Serve {
                reason,
                category_label,
            } => {
                assert_eq!(reason, "category_threshold_and_mismatch_ok");
                assert_eq!(category_label, "code");
            }
            other => panic!("expected serve, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn warm_high_cost_region_rejects() {
        let policy = policy_with_threshold(0.5, Duration::from_secs(60));
        let learner = MismatchCostLearner::new(MismatchConfig {
            similarity_floor: 0.5,
            allowed_cost: 0.1,
            decay_seconds: 3600,
        });
        // Warm up with high observed cost so the learner rejects.
        for _ in 0..50 {
            learner.observe(MismatchRegion::new("t", "code"), 0.4).await;
        }
        let gate = ResultCacheGate::new(&policy, &learner);
        let d = gate
            .decide(&GateInputs {
                category: "code",
                tenant_id: "t",
                similarity: 0.95,
                age: Duration::from_secs(1),
            })
            .await;
        match d {
            GateDecision::Reject { reason } => assert_eq!(reason, "above_allowed_cost"),
            other => panic!("expected above_allowed_cost reject, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn unknown_category_uses_safe_fallback_policy() {
        // The default fallback policy has similarity_threshold = 0.78 +
        // TTL of 60s. An unknown category must NOT crash; it falls through.
        let policy = PerCategoryPolicy::with_defaults();
        let learner = warm_learner_for_low_cost();
        let gate = ResultCacheGate::new(&policy, &learner);
        // Similarity below the fallback threshold rejects.
        let d = gate
            .decide(&GateInputs {
                category: "definitely-new-category",
                tenant_id: "t",
                similarity: 0.5,
                age: Duration::from_secs(1),
            })
            .await;
        assert!(!d.is_serve(), "below-fallback-threshold should not serve");
    }

    #[tokio::test]
    async fn same_inputs_different_tenants_yield_independent_decisions() {
        // Cross-tenant isolation at the mismatch-region level.
        let policy = policy_with_threshold(0.5, Duration::from_secs(60));
        let learner = warm_learner_for_low_cost();
        // Warm tenant-a only.
        for _ in 0..50 {
            learner
                .observe(MismatchRegion::new("tenant-a", "code"), 0.05)
                .await;
        }
        let gate = ResultCacheGate::new(&policy, &learner);
        let a = gate
            .decide(&GateInputs {
                category: "code",
                tenant_id: "tenant-a",
                similarity: 0.95,
                age: Duration::from_secs(1),
            })
            .await;
        let b = gate
            .decide(&GateInputs {
                category: "code",
                tenant_id: "tenant-b",
                similarity: 0.95,
                age: Duration::from_secs(1),
            })
            .await;
        assert!(a.is_serve(), "tenant-a is warm and should serve");
        assert!(!b.is_serve(), "tenant-b is cold and must not serve");
    }

    #[tokio::test]
    async fn boundary_similarity_at_threshold_passes_category_check() {
        // Similarity exactly equal to the category threshold passes the
        // category gate (we cleared the "strict less than" bug).
        let policy = policy_with_threshold(0.8, Duration::from_secs(60));
        let learner = warm_learner_for_low_cost();
        // Pre-warm so we get past the cold-region check.
        for _ in 0..50 {
            learner
                .observe(MismatchRegion::new("t", "code"), 0.05)
                .await;
        }
        let gate = ResultCacheGate::new(&policy, &learner);
        let d = gate
            .decide(&GateInputs {
                category: "code",
                tenant_id: "t",
                similarity: 0.8,
                age: Duration::from_secs(1),
            })
            .await;
        assert!(d.is_serve(), "exact threshold should pass category gate");
    }

    #[tokio::test]
    async fn boundary_age_at_ttl_passes() {
        // age == ttl is still alive; strictly greater rejects.
        let policy = policy_with_threshold(0.5, Duration::from_secs(60));
        let learner = warm_learner_for_low_cost();
        for _ in 0..50 {
            learner
                .observe(MismatchRegion::new("t", "code"), 0.05)
                .await;
        }
        let gate = ResultCacheGate::new(&policy, &learner);
        let d = gate
            .decide(&GateInputs {
                category: "code",
                tenant_id: "t",
                similarity: 0.95,
                age: Duration::from_secs(60),
            })
            .await;
        assert!(d.is_serve(), "exact TTL boundary should pass");
    }

    #[tokio::test]
    async fn is_serve_helper_works() {
        let serve = GateDecision::Serve {
            reason: "x",
            category_label: "code",
        };
        let reject = GateDecision::Reject {
            reason: "ttl_expired",
        };
        assert!(serve.is_serve());
        assert!(!reject.is_serve());
    }
}
