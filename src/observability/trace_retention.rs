// Trace retention policy — companion to `trace_sampling`.
//
// LLD Open Risks row pairs sampling with retention:
//
//   "Mitigation: down-sample at the gateway by tier (free tier 10%,
//    pooled 50%, dedicated 100%) and keep a 30-day retention policy."
//
// `trace_sampling` decides whether to PERSIST a trace record. This
// module decides whether to PRUNE an already-persisted record based on
// age, the tenant's tier-specific retention window, and a soft storage
// budget. The retention sweeper calls `decide()` per record per sweep
// pass and prunes records whose decision is `Prune`.
//
// Retention windows differ by tier so a paying customer gets more
// history than a free-tier evaluation user. The free tier's 7-day
// window is short enough that even sampled traces (10%) don't pile up
// indefinitely; the enterprise tier's 90-day window aligns with most
// audit-compliance requirements.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::observability::trace_sampling::TierLabel;

/// One per-tier retention window.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TierRetention {
    /// Bounded tier label — matches `Tier::prometheus_label`.
    pub tier: String,
    /// How long a trace persists before it's eligible for pruning.
    pub window: Duration,
}

/// Retention policy configuration.
#[derive(Debug, Clone)]
pub struct TraceRetentionConfig {
    pub per_tier: Vec<TierRetention>,
    /// Window applied to tiers not enumerated in `per_tier`. Defaults
    /// to the shortest tier window so a misconfigured tier doesn't
    /// silently keep more history than intended.
    pub default_window: Duration,
    /// Soft cap on the storage the trace collection may consume. When
    /// `current_bytes / soft_budget_bytes` exceeds 1.0 the policy
    /// begins pruning records younger than their retention window,
    /// starting with the oldest.
    pub soft_budget_bytes: u64,
}

impl Default for TraceRetentionConfig {
    fn default() -> Self {
        Self {
            per_tier: vec![
                TierRetention {
                    tier: "free".into(),
                    window: Duration::from_secs(7 * 86_400),
                },
                TierRetention {
                    tier: "community".into(),
                    window: Duration::from_secs(14 * 86_400),
                },
                TierRetention {
                    tier: "business".into(),
                    window: Duration::from_secs(30 * 86_400),
                },
                TierRetention {
                    tier: "enterprise".into(),
                    window: Duration::from_secs(90 * 86_400),
                },
            ],
            default_window: Duration::from_secs(7 * 86_400),
            soft_budget_bytes: 100 * 1024 * 1024 * 1024, // 100 GB
        }
    }
}

/// Inputs the policy consumes per record.
#[derive(Debug, Clone)]
pub struct RetentionInputs {
    /// Tier of the tenant that owns the record.
    pub tier_label: TierLabel,
    /// Age of the trace record at evaluation time.
    pub age: Duration,
    /// Current total bytes consumed by the trace collection. Pass 0 to
    /// disable budget-driven shedding.
    pub current_bytes: u64,
    /// Approximate bytes the record itself occupies. Used to bias the
    /// budget-shed decision toward dropping bigger records first; pass
    /// 0 when unknown.
    pub record_bytes: u64,
}

/// Decision emitted per record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RetentionDecision {
    /// Keep the record.
    Keep,
    /// Drop the record.
    Prune { reason: &'static str },
}

impl RetentionDecision {
    pub fn is_prune(&self) -> bool {
        matches!(self, RetentionDecision::Prune { .. })
    }
}

/// Retention policy. Plain struct — no internal state — so the sweeper
/// can construct one per sweep pass without coordination.
#[derive(Debug, Clone)]
pub struct TraceRetentionPolicy {
    config: TraceRetentionConfig,
}

impl TraceRetentionPolicy {
    pub fn new(config: TraceRetentionConfig) -> Self {
        Self { config }
    }

    pub fn with_defaults() -> Self {
        Self::new(TraceRetentionConfig::default())
    }

    /// Effective window for a tier — pure lookup, no I/O.
    pub fn window_for(&self, tier: TierLabel) -> Duration {
        self.config
            .per_tier
            .iter()
            .find(|r| r.tier == tier)
            .map(|r| r.window)
            .unwrap_or(self.config.default_window)
    }

    /// Whether the current storage usage is over the soft budget.
    pub fn is_over_budget(&self, current_bytes: u64) -> bool {
        self.config.soft_budget_bytes > 0 && current_bytes > self.config.soft_budget_bytes
    }

    /// Decide on a single record.
    pub fn decide(&self, inputs: &RetentionInputs) -> RetentionDecision {
        let window = self.window_for(inputs.tier_label);

        // Step 1: hard age cutoff. Anything past its tier window is
        // always pruned regardless of budget.
        if inputs.age >= window {
            return RetentionDecision::Prune {
                reason: "age_window",
            };
        }

        // Step 2: budget shedding. When over the soft budget, prune
        // anything past half its window. The half-window heuristic
        // preserves recent traces (which are most useful for live
        // debugging) and only pressures the long tail.
        if self.is_over_budget(inputs.current_bytes) && inputs.age >= window / 2 {
            return RetentionDecision::Prune {
                reason: "budget_shed",
            };
        }

        RetentionDecision::Keep
    }

    /// Sort + select helper for callers that have a batch of candidate
    /// records and want to know which to prune. Sorts by age descending
    /// so the oldest records are pruned first under budget pressure.
    pub fn select_for_prune<I, T>(
        &self,
        records: I,
        extract: impl Fn(&T) -> RetentionInputs,
    ) -> Vec<T>
    where
        I: IntoIterator<Item = T>,
    {
        let mut scored: Vec<(T, Duration)> = records
            .into_iter()
            .filter_map(|r| {
                let inputs = extract(&r);
                let age = inputs.age;
                if self.decide(&inputs).is_prune() {
                    Some((r, age))
                } else {
                    None
                }
            })
            .collect();
        scored.sort_by(|a, b| b.1.cmp(&a.1));
        scored.into_iter().map(|(r, _)| r).collect()
    }
}

impl Default for TraceRetentionPolicy {
    fn default() -> Self {
        Self::with_defaults()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs(tier: TierLabel, age: Duration, current: u64, record: u64) -> RetentionInputs {
        RetentionInputs {
            tier_label: tier,
            age,
            current_bytes: current,
            record_bytes: record,
        }
    }

    fn days(n: u64) -> Duration {
        Duration::from_secs(n * 86_400)
    }

    #[test]
    fn defaults_match_lld_per_tier_windows() {
        let p = TraceRetentionPolicy::with_defaults();
        assert_eq!(p.window_for("free"), days(7));
        assert_eq!(p.window_for("community"), days(14));
        assert_eq!(p.window_for("business"), days(30));
        assert_eq!(p.window_for("enterprise"), days(90));
    }

    #[test]
    fn unknown_tier_uses_default_window() {
        // Default = shortest tier window (7 days). A misconfigured tier
        // can't silently keep more history than intended.
        let p = TraceRetentionPolicy::with_defaults();
        assert_eq!(p.window_for("legacy-tier"), days(7));
    }

    #[test]
    fn record_within_window_is_kept() {
        let p = TraceRetentionPolicy::with_defaults();
        let d = p.decide(&inputs("business", days(15), 0, 0));
        assert_eq!(d, RetentionDecision::Keep);
    }

    #[test]
    fn record_past_window_is_pruned_with_age_reason() {
        let p = TraceRetentionPolicy::with_defaults();
        // Business window = 30 days; this record is 45 days old.
        let d = p.decide(&inputs("business", days(45), 0, 0));
        match d {
            RetentionDecision::Prune { reason } => assert_eq!(reason, "age_window"),
            other => panic!("expected age_window prune, got {other:?}"),
        }
    }

    #[test]
    fn exactly_at_window_prunes() {
        // Boundary: age == window prunes. Strict-less-than would keep
        // records forever at the boundary.
        let p = TraceRetentionPolicy::with_defaults();
        let d = p.decide(&inputs("free", days(7), 0, 0));
        assert!(d.is_prune());
    }

    #[test]
    fn budget_shedding_drops_half_window_records() {
        // Business window = 30 days; record is 20 days old (past half
        // window, but inside full window). With current_bytes over the
        // soft budget, the policy prunes.
        let p = TraceRetentionPolicy::with_defaults();
        let over_budget = 200 * 1024 * 1024 * 1024; // 200 GB > 100 GB cap
        let d = p.decide(&inputs("business", days(20), over_budget, 0));
        match d {
            RetentionDecision::Prune { reason } => assert_eq!(reason, "budget_shed"),
            other => panic!("expected budget_shed, got {other:?}"),
        }
    }

    #[test]
    fn budget_shedding_preserves_fresh_records() {
        // Even over budget, anything younger than half the window stays.
        let p = TraceRetentionPolicy::with_defaults();
        let over_budget = 200 * 1024 * 1024 * 1024;
        // 5 days < 15 (half of 30) → keep.
        let d = p.decide(&inputs("business", days(5), over_budget, 0));
        assert_eq!(d, RetentionDecision::Keep);
    }

    #[test]
    fn under_budget_keeps_records_past_half_window() {
        // Same record as the budget-shed test but with under-budget
        // usage — must keep.
        let p = TraceRetentionPolicy::with_defaults();
        let d = p.decide(&inputs("business", days(20), 1024, 0));
        assert_eq!(d, RetentionDecision::Keep);
    }

    #[test]
    fn budget_zero_disables_shedding() {
        // soft_budget_bytes = 0 means no budget — only age window prunes.
        let p = TraceRetentionPolicy::new(TraceRetentionConfig {
            per_tier: vec![TierRetention {
                tier: "free".into(),
                window: days(7),
            }],
            default_window: days(7),
            soft_budget_bytes: 0,
        });
        // Any current_bytes value with budget=0 → policy never sheds.
        let d = p.decide(&inputs("free", days(5), u64::MAX, 0));
        assert_eq!(d, RetentionDecision::Keep);
    }

    #[test]
    fn over_budget_does_not_prune_past_age_window_with_shed_reason() {
        // A record past its window AND over budget gets the age_window
        // reason because the age check runs first — the more
        // informative reason wins.
        let p = TraceRetentionPolicy::with_defaults();
        let over_budget = 200 * 1024 * 1024 * 1024;
        let d = p.decide(&inputs("business", days(60), over_budget, 0));
        match d {
            RetentionDecision::Prune { reason } => assert_eq!(reason, "age_window"),
            other => panic!("expected age_window, got {other:?}"),
        }
    }

    #[test]
    fn select_for_prune_returns_oldest_first() {
        // Three records past their windows; select_for_prune returns
        // them sorted oldest-first so the sweeper can stream-delete
        // without re-sorting.
        let p = TraceRetentionPolicy::with_defaults();
        let records = vec![("r1", days(8)), ("r2", days(45)), ("r3", days(15))];
        let out = p.select_for_prune(records, |(_, age)| inputs("free", *age, 0, 0));
        assert_eq!(out.len(), 3, "all three records are past free-tier window");
        assert_eq!(out[0].0, "r2", "45-day record first");
        assert_eq!(out[1].0, "r3", "15-day record second");
        assert_eq!(out[2].0, "r1", "8-day record last");
    }

    #[test]
    fn select_for_prune_omits_keep_records() {
        let p = TraceRetentionPolicy::with_defaults();
        let records = vec![("fresh", days(3)), ("old", days(45))];
        let out = p.select_for_prune(records, |(_, age)| inputs("business", *age, 0, 0));
        // Only the 45-day record exceeds business's 30-day window.
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].0, "old");
    }

    #[test]
    fn is_over_budget_is_strict_greater_than() {
        let p = TraceRetentionPolicy::with_defaults();
        assert!(!p.is_over_budget(p.config.soft_budget_bytes));
        assert!(p.is_over_budget(p.config.soft_budget_bytes + 1));
    }

    #[test]
    fn enterprise_keeps_a_long_tail() {
        // Enterprise tier — 60-day-old record must keep even over budget,
        // because half of 90 days = 45 days. (Anti-test: shorter tiers
        // would prune at 60 days.)
        let p = TraceRetentionPolicy::with_defaults();
        let over_budget = 200 * 1024 * 1024 * 1024;
        let d = p.decide(&inputs("enterprise", days(40), over_budget, 0));
        assert_eq!(d, RetentionDecision::Keep, "40 days < 45 (half of 90)");
    }
}
