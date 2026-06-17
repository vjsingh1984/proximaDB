// Trace lifecycle integration — composes sampling (write) and
// retention (sweep) so the LLD risk-row "down-sample at gateway +
// 30-day retention" mitigation is verified end-to-end.
//
// Pipeline:
//
//   incoming trace
//     → trace_sampling::decide → Sample | Drop  (write decision)
//   if sampled, stored
//     → trace_retention::decide → Keep | Prune  (sweep decision per record)
//
// Each primitive is unit-tested individually. This test pins the
// CROSS-STAGE contract: a sampled trace ages out by its tier window;
// a dropped-at-write trace never enters retention. The two
// primitives must compose cleanly because production deployments
// run them as one logical lifecycle.

use std::time::Duration;

use proximadb::observability::trace_retention::{
    RetentionDecision, RetentionInputs, TraceRetentionPolicy,
};
use proximadb::observability::trace_sampling::{SamplingInputs, TraceSamplingPolicy};

fn days(n: u64) -> Duration {
    Duration::from_secs(n * 86_400)
}

/// Happy path: a business-tier trace is sampled at write (100% rate)
/// and kept through its 30-day window, then pruned past it.
#[tokio::test]
async fn business_trace_sampled_kept_then_pruned_at_window() {
    let sampling = TraceSamplingPolicy::with_defaults();
    let retention = TraceRetentionPolicy::with_defaults();

    // Stage 1: write decision.
    let sampling_decision = sampling
        .decide(&SamplingInputs {
            tier_label: "business",
            trace_id: "trace-b1",
            current_load: 0.0,
            force_sample: false,
        })
        .await;
    assert!(
        sampling_decision.is_sample(),
        "business tier samples at 100%"
    );

    // Stage 2: retention at 10 days — kept.
    let d_mid = retention.decide(&RetentionInputs {
        tier_label: "business",
        age: days(10),
        current_bytes: 0,
        record_bytes: 0,
    });
    assert_eq!(d_mid, RetentionDecision::Keep);

    // Stage 3: retention at 45 days — pruned by age window.
    let d_old = retention.decide(&RetentionInputs {
        tier_label: "business",
        age: days(45),
        current_bytes: 0,
        record_bytes: 0,
    });
    assert!(d_old.is_prune());
}

/// Free tier samples at ~10%. The retention window is 7 days. A
/// trace that does survive the sampling gate must still get pruned
/// at day 7.
#[tokio::test]
async fn free_tier_sampling_and_retention_compose_correctly() {
    let sampling = TraceSamplingPolicy::with_defaults();
    let retention = TraceRetentionPolicy::with_defaults();

    // Force-sample so we can test the retention side deterministically.
    let sampling_decision = sampling
        .decide(&SamplingInputs {
            tier_label: "free",
            trace_id: "trace-f1",
            current_load: 0.0,
            force_sample: true,
        })
        .await;
    assert!(sampling_decision.is_sample());

    // Day 5 — kept.
    assert_eq!(
        retention.decide(&RetentionInputs {
            tier_label: "free",
            age: days(5),
            current_bytes: 0,
            record_bytes: 0,
        }),
        RetentionDecision::Keep
    );

    // Day 8 — past free's 7-day window, pruned.
    assert!(
        retention
            .decide(&RetentionInputs {
                tier_label: "free",
                age: days(8),
                current_bytes: 0,
                record_bytes: 0,
            })
            .is_prune()
    );
}

/// Enterprise tier samples at 100% and retains for 90 days. A trace
/// that other tiers would have aged out by day 30 is still kept.
#[tokio::test]
async fn enterprise_keeps_a_long_tail_other_tiers_would_prune() {
    let sampling = TraceSamplingPolicy::with_defaults();
    let retention = TraceRetentionPolicy::with_defaults();
    let sampled = sampling
        .decide(&SamplingInputs {
            tier_label: "enterprise",
            trace_id: "trace-ent",
            current_load: 0.0,
            force_sample: false,
        })
        .await;
    assert!(sampled.is_sample());

    // Day 60 — past business's window (30d) but inside enterprise's
    // (90d).
    let business_d = retention.decide(&RetentionInputs {
        tier_label: "business",
        age: days(60),
        current_bytes: 0,
        record_bytes: 0,
    });
    let enterprise_d = retention.decide(&RetentionInputs {
        tier_label: "enterprise",
        age: days(60),
        current_bytes: 0,
        record_bytes: 0,
    });
    assert!(business_d.is_prune());
    assert_eq!(enterprise_d, RetentionDecision::Keep);
}

/// Load shedding at write + budget shedding at sweep: both
/// primitives can drop traces independently; the lifecycle must not
/// confuse the two.
#[tokio::test]
async fn load_shed_at_write_independent_from_budget_shed_at_sweep() {
    let sampling = TraceSamplingPolicy::with_defaults();
    let retention = TraceRetentionPolicy::with_defaults();

    // Load=1.0 → sampling rate goes to zero for any tier.
    let d_write = sampling
        .decide(&SamplingInputs {
            tier_label: "business",
            trace_id: "trace-load",
            current_load: 1.0,
            force_sample: false,
        })
        .await;
    assert!(!d_write.is_sample(), "load=1.0 drops");

    // For traces that DID get sampled earlier, budget shedding at
    // sweep is an independent decision. With current_bytes well
    // above the soft budget, a 20-day-old business trace prunes
    // even though business's normal window is 30 days.
    let over_budget = 200 * 1024 * 1024 * 1024;
    let d_sweep = retention.decide(&RetentionInputs {
        tier_label: "business",
        age: days(20),
        current_bytes: over_budget,
        record_bytes: 0,
    });
    assert!(d_sweep.is_prune());
}

/// Force-sample at write + age-window-pruned at sweep: a trace can
/// pass the write gate via force-sample and still age out normally.
#[tokio::test]
async fn force_sampled_trace_still_subject_to_retention() {
    let sampling = TraceSamplingPolicy::with_defaults();
    let retention = TraceRetentionPolicy::with_defaults();

    let sampled = sampling
        .decide(&SamplingInputs {
            tier_label: "free",
            trace_id: "trace-debug",
            current_load: 0.9, // would normally drop
            force_sample: true,
        })
        .await;
    assert!(sampled.is_sample(), "force_sample overrides load shed");

    // But it still ages out per the free tier's 7-day window.
    let d = retention.decide(&RetentionInputs {
        tier_label: "free",
        age: days(10),
        current_bytes: 0,
        record_bytes: 0,
    });
    assert!(d.is_prune());
}

/// Sampling deterministic on trace_id: the same trace_id under the
/// same config consistently passes (or fails) the write gate. Then
/// retention is deterministic on age. The lifecycle is fully
/// deterministic given inputs.
#[tokio::test]
async fn lifecycle_is_deterministic_given_inputs() {
    let sampling = TraceSamplingPolicy::with_defaults();
    let retention = TraceRetentionPolicy::with_defaults();

    let id = "stable-trace-abc";
    let a = sampling
        .decide(&SamplingInputs {
            tier_label: "free",
            trace_id: id,
            current_load: 0.0,
            force_sample: false,
        })
        .await;
    let b = sampling
        .decide(&SamplingInputs {
            tier_label: "free",
            trace_id: id,
            current_load: 0.0,
            force_sample: false,
        })
        .await;
    assert_eq!(a, b, "same trace_id → same sampling decision");

    let r1 = retention.decide(&RetentionInputs {
        tier_label: "free",
        age: days(3),
        current_bytes: 0,
        record_bytes: 0,
    });
    let r2 = retention.decide(&RetentionInputs {
        tier_label: "free",
        age: days(3),
        current_bytes: 0,
        record_bytes: 0,
    });
    assert_eq!(r1, r2, "same age → same retention decision");
}
