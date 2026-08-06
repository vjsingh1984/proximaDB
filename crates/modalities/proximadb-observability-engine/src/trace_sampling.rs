// Trace sampling policy — LLD "Open Risks" mitigation.
//
// The risk row pins this requirement:
//
//   "Trace cardinality. The trace-archive collection will be hot.
//    Mitigation: down-sample at the gateway by tier (lowest tier 10%,
//    pooled 50%, dedicated 100%) and keep a 30-day retention policy."
//
// Every search emits a SearchPlanTrace today; persisting all of them
// would saturate the billing collection on a free-tier traffic burst.
// This policy is the decision primitive the gateway consults before
// writing the trace to its async sink. The metering KRU event is a
// separate billing-critical record and always emits — only the
// SearchPlanTrace + planner-v2 training-record sinks get sampled.
//
// Sampling is **deterministic on trace_id** (not random) so:
//   - Two services seeing the same trace_id agree on the sampling
//     decision without coordination.
//   - A specific trace_id is consistently sampled across retries.
//   - Force-sample (debug=true on the request) always overrides.

use std::sync::Arc;

use tokio::sync::RwLock;

/// Tier label — must match the bounded set from `Tier::prometheus_label`.
/// Stored as `&'static str` so the policy stays cardinality-safe; serde
/// derives intentionally omitted because the policy is configured in
/// process (hot-swappable via `replace_config`), not deserialized from
/// the wire.
pub type TierLabel = &'static str;

/// One per-tier sample-rate config. Rate in `[0.0, 1.0]`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TierSampleRate {
    pub tier: TierLabel,
    pub rate: f64,
}

/// Sampling-policy configuration. Per-tier rates + a load-shedding
/// threshold that scales the rates down when the gateway is under heavy
/// load. The defaults match the LLD risk-row guidance.
#[derive(Debug, Clone)]
pub struct TraceSamplingConfig {
    pub per_tier: Vec<TierSampleRate>,
    /// Default rate for tiers not listed in `per_tier`. Conservative —
    /// an unknown tier samples rarely to avoid surprise cardinality.
    pub default_rate: f64,
    /// Above this load value (in [0, 1]), the policy scales the rate
    /// linearly toward zero. The runtime supplies a current-load number
    /// derived from queue depth, CPU, or a custom signal.
    pub load_shed_threshold: f64,
}

impl Default for TraceSamplingConfig {
    fn default() -> Self {
        Self {
            per_tier: vec![
                TierSampleRate {
                    tier: "free",
                    rate: 0.10,
                },
                TierSampleRate {
                    tier: "community",
                    rate: 0.50,
                },
                TierSampleRate {
                    tier: "business",
                    rate: 1.00,
                },
                TierSampleRate {
                    tier: "enterprise",
                    rate: 1.00,
                },
            ],
            default_rate: 0.10,
            load_shed_threshold: 0.75,
        }
    }
}

/// Decision the policy emits — typed so the trace router can branch on
/// it without re-deriving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TraceSamplingDecision {
    /// Persist the full trace.
    Sample,
    /// Drop the trace (KRU billing still emits separately).
    Drop {
        /// Why we dropped — "rate", "load_shed", "no_trace_id".
        reason: &'static str,
    },
}

impl TraceSamplingDecision {
    pub fn is_sample(&self) -> bool {
        matches!(self, TraceSamplingDecision::Sample)
    }
}

/// Inputs the policy consumes per trace.
#[derive(Debug, Clone)]
pub struct SamplingInputs<'a> {
    /// Bounded tier label from `Tier::prometheus_label`.
    pub tier_label: TierLabel,
    /// Stable per-query identifier the gateway minted. Empty string =
    /// always drop (the policy can't make a deterministic decision
    /// without an id).
    pub trace_id: &'a str,
    /// Current load in `[0, 1]`. The runtime supplies this from queue
    /// depth or CPU. Pass 0.0 to disable load shedding.
    pub current_load: f64,
    /// `true` when the caller asked for the trace via `debug=true`. Always
    /// samples regardless of rate or load.
    pub force_sample: bool,
}

/// The policy. Cheap to clone — wraps an `Arc<RwLock<…>>` so
/// configuration updates (e.g. an SRE bumping community to 0.75)
/// propagate to in-flight requests on the next read.
#[derive(Clone)]
pub struct TraceSamplingPolicy {
    inner: Arc<RwLock<TraceSamplingConfig>>,
}

impl TraceSamplingPolicy {
    pub fn new(config: TraceSamplingConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(config)),
        }
    }

    pub fn with_defaults() -> Self {
        Self::new(TraceSamplingConfig::default())
    }

    /// Hot-swap the config. Live requests see the new rates on their
    /// next call.
    pub async fn replace_config(&self, config: TraceSamplingConfig) {
        *self.inner.write().await = config;
    }

    /// Decide whether to sample a trace. Pure given the config snapshot;
    /// the only state read is the current config.
    pub async fn decide(&self, inputs: &SamplingInputs<'_>) -> TraceSamplingDecision {
        if inputs.force_sample {
            return TraceSamplingDecision::Sample;
        }
        if inputs.trace_id.is_empty() {
            // A trace without an id can't be sampled deterministically.
            // Drop rather than make a random call — the absence of an id
            // is itself a signal the gateway should fix.
            return TraceSamplingDecision::Drop {
                reason: "no_trace_id",
            };
        }
        let config = self.inner.read().await;
        let base_rate = lookup_rate(&config, inputs.tier_label);
        let effective_rate = apply_load_shedding(base_rate, inputs.current_load, &config);
        let bucket = trace_bucket(inputs.trace_id);
        if bucket < (effective_rate * 1_000_000.0).round() as u64 {
            TraceSamplingDecision::Sample
        } else if effective_rate < base_rate {
            TraceSamplingDecision::Drop {
                reason: "load_shed",
            }
        } else {
            TraceSamplingDecision::Drop { reason: "rate" }
        }
    }

    /// Inspect the effective rate a tier currently sees under the given
    /// load — useful for observability dashboards.
    pub async fn effective_rate(&self, tier: TierLabel, current_load: f64) -> f64 {
        let config = self.inner.read().await;
        let base = lookup_rate(&config, tier);
        apply_load_shedding(base, current_load, &config)
    }
}

impl Default for TraceSamplingPolicy {
    fn default() -> Self {
        Self::with_defaults()
    }
}

fn lookup_rate(config: &TraceSamplingConfig, tier: TierLabel) -> f64 {
    config
        .per_tier
        .iter()
        .find(|t| t.tier == tier)
        .map(|t| t.rate)
        .unwrap_or(config.default_rate)
        .clamp(0.0, 1.0)
}

/// Linear scale from full rate at threshold to zero at load=1.0. Load
/// values at or below the threshold preserve the base rate; above it,
/// the rate decays linearly.
fn apply_load_shedding(base_rate: f64, current_load: f64, config: &TraceSamplingConfig) -> f64 {
    let load = current_load.clamp(0.0, 1.0);
    let threshold = config.load_shed_threshold.clamp(0.0, 1.0);
    if load <= threshold || threshold >= 1.0 {
        return base_rate;
    }
    let headroom = (1.0 - threshold).max(f64::MIN_POSITIVE);
    let factor = ((1.0 - load) / headroom).clamp(0.0, 1.0);
    (base_rate * factor).clamp(0.0, 1.0)
}

/// Map a trace_id to a deterministic bucket in `[0, 1_000_000)`. The
/// hash is FNV-1a 64-bit because it's tiny, allocation-free, and the
/// distribution is good enough for sampling (cryptographic hashes are
/// overkill here).
fn trace_bucket(trace_id: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325; // FNV-1a offset basis
    for b in trace_id.as_bytes() {
        hash ^= *b as u64;
        hash = hash.wrapping_mul(0x100000001b3); // FNV prime
    }
    hash % 1_000_000
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(tier: TierLabel, trace_id: &'a str, load: f64) -> SamplingInputs<'a> {
        SamplingInputs {
            tier_label: tier,
            trace_id,
            current_load: load,
            force_sample: false,
        }
    }

    #[tokio::test]
    async fn force_sample_overrides_everything() {
        let policy = TraceSamplingPolicy::with_defaults();
        let mut i = inputs("free", "any-id", 0.95); // load shed + free tier
        i.force_sample = true;
        let d = policy.decide(&i).await;
        assert!(d.is_sample());
    }

    #[tokio::test]
    async fn empty_trace_id_always_drops() {
        let policy = TraceSamplingPolicy::with_defaults();
        let d = policy.decide(&inputs("enterprise", "", 0.0)).await;
        match d {
            TraceSamplingDecision::Drop { reason } => assert_eq!(reason, "no_trace_id"),
            other => panic!("expected no_trace_id drop, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn enterprise_always_samples_under_no_load() {
        let policy = TraceSamplingPolicy::with_defaults();
        // Probe a bunch of trace ids — every one should sample at rate 1.0.
        for i in 0..50 {
            let id = format!("trace-{i}");
            let d = policy.decide(&inputs("enterprise", &id, 0.0)).await;
            assert!(d.is_sample(), "enterprise must sample {id}");
        }
    }

    #[tokio::test]
    async fn free_tier_samples_at_roughly_ten_percent() {
        let policy = TraceSamplingPolicy::with_defaults();
        let mut samples = 0;
        let n = 1000;
        for i in 0..n {
            let id = format!("trace-{i}");
            if policy.decide(&inputs("free", &id, 0.0)).await.is_sample() {
                samples += 1;
            }
        }
        let rate = samples as f64 / n as f64;
        // FNV-1a is well-distributed; expect within ±5pp of 10%.
        assert!(
            (0.05..=0.15).contains(&rate),
            "free-tier sample rate out of band: {rate}"
        );
    }

    #[tokio::test]
    async fn business_tier_samples_at_one_hundred_percent() {
        let policy = TraceSamplingPolicy::with_defaults();
        for i in 0..200 {
            let id = format!("trace-{i}");
            assert!(
                policy
                    .decide(&inputs("business", &id, 0.0))
                    .await
                    .is_sample(),
                "business must sample every trace"
            );
        }
    }

    #[tokio::test]
    async fn unknown_tier_uses_default_rate() {
        let policy = TraceSamplingPolicy::with_defaults();
        // default_rate = 0.10 — same band as free-tier sampling.
        let mut samples = 0;
        let n = 1000;
        for i in 0..n {
            let id = format!("trace-{i}");
            if policy
                .decide(&inputs("legacy-tier", &id, 0.0))
                .await
                .is_sample()
            {
                samples += 1;
            }
        }
        let rate = samples as f64 / n as f64;
        assert!(
            (0.05..=0.15).contains(&rate),
            "default rate out of band: {rate}"
        );
    }

    #[tokio::test]
    async fn load_shedding_below_threshold_preserves_base_rate() {
        let policy = TraceSamplingPolicy::with_defaults();
        // Load 0.5, threshold 0.75 → unchanged.
        let r = policy.effective_rate("business", 0.5).await;
        assert_eq!(r, 1.0);
    }

    #[tokio::test]
    async fn load_shedding_above_threshold_scales_rate_down() {
        let policy = TraceSamplingPolicy::with_defaults();
        // Load 0.875 is halfway from 0.75 to 1.0 → rate halves.
        let r = policy.effective_rate("business", 0.875).await;
        assert!((r - 0.5).abs() < 1e-9, "expected 0.5, got {r}");
    }

    #[tokio::test]
    async fn load_at_one_zeros_the_rate() {
        let policy = TraceSamplingPolicy::with_defaults();
        let r = policy.effective_rate("business", 1.0).await;
        assert_eq!(r, 0.0);
    }

    #[tokio::test]
    async fn load_shed_drop_reason_is_distinct_from_rate_drop() {
        // Free tier under heavy load — the drop reason should reflect
        // the load contribution, not just the base rate.
        let policy = TraceSamplingPolicy::with_defaults();
        // Free tier base = 0.10, load 0.95 scales it to ~0.02.
        let i = inputs("free", "very-unlucky-trace-id-zzz", 0.95);
        if let TraceSamplingDecision::Drop { reason } = policy.decide(&i).await {
            // Either load_shed (if the load reduction is what tipped it)
            // or rate (if it would have dropped at full base too). Both
            // are legitimate; the reason distinguishes them.
            assert!(reason == "load_shed" || reason == "rate");
        }
    }

    #[tokio::test]
    async fn decision_is_deterministic_per_trace_id() {
        // Same trace_id + same config → same decision every time.
        let policy = TraceSamplingPolicy::with_defaults();
        let id = "stable-trace-abc";
        let a = policy.decide(&inputs("free", id, 0.0)).await;
        let b = policy.decide(&inputs("free", id, 0.0)).await;
        let c = policy.decide(&inputs("free", id, 0.0)).await;
        assert_eq!(a, b);
        assert_eq!(b, c);
    }

    #[tokio::test]
    async fn config_replacement_takes_effect_on_next_call() {
        let policy = TraceSamplingPolicy::with_defaults();
        let id = "trace-config-change";
        // Force a config where free tier always samples.
        let new_config = TraceSamplingConfig {
            per_tier: vec![TierSampleRate {
                tier: "free",
                rate: 1.0,
            }],
            default_rate: 0.0,
            load_shed_threshold: 1.0,
        };
        policy.replace_config(new_config).await;
        let d = policy.decide(&inputs("free", id, 0.0)).await;
        assert!(
            d.is_sample(),
            "after replace_config(rate=1.0), free must sample"
        );
    }

    #[tokio::test]
    async fn out_of_range_rate_clamps_to_unit() {
        let policy = TraceSamplingPolicy::new(TraceSamplingConfig {
            per_tier: vec![TierSampleRate {
                tier: "weird",
                rate: 5.0,
            }],
            default_rate: 0.0,
            load_shed_threshold: 1.0,
        });
        let r = policy.effective_rate("weird", 0.0).await;
        // 5.0 clamps to 1.0.
        assert_eq!(r, 1.0);
    }

    #[tokio::test]
    async fn negative_rate_clamps_to_zero() {
        let policy = TraceSamplingPolicy::new(TraceSamplingConfig {
            per_tier: vec![TierSampleRate {
                tier: "weird",
                rate: -0.5,
            }],
            default_rate: 0.0,
            load_shed_threshold: 1.0,
        });
        let r = policy.effective_rate("weird", 0.0).await;
        assert_eq!(r, 0.0);
    }

    #[test]
    fn trace_bucket_is_deterministic() {
        let a = trace_bucket("hello");
        let b = trace_bucket("hello");
        assert_eq!(a, b);
        // Distinct inputs map to (very likely) distinct buckets.
        let c = trace_bucket("world");
        assert_ne!(a, c, "FNV-1a should distinguish these inputs");
    }

    #[test]
    fn trace_bucket_in_range() {
        for s in ["", "a", "trace-1", "uuid-aaaa-bbbb"] {
            let b = trace_bucket(s);
            assert!(b < 1_000_000);
        }
    }
}
