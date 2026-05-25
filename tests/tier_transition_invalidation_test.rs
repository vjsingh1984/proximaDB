// Tier transition + cache invalidation integration.
//
// The tenant lifecycle: a tier change must (1) emit a structured
// transition event, (2) flush caches that held the old budget, so
// the next planner call recomputes against the new tier's caps.
// Each primitive ships TDD; this proves the chain.
//
// Pipeline:
//
//   before snapshot + after snapshot
//     → tier_transition::detect → TierTransitionEvent
//     → invalidation_coordinator::invalidate_collection(tenant, collection)
//     → next planner call: PlanCache miss (was hit before)

use std::sync::Arc;

use proximadb::catalog::tenant_tier::{FeatureFlags, Tier, TenantTierRecord};
use proximadb::catalog::tier_transition::{
    AxisDirection, TransitionClass, detect as detect_transition,
};
use proximadb::query::cache::invalidation_coordinator::CacheInvalidationCoordinator;
use proximadb::query::cache::plan_cache::PlanCache;
use proximadb::query::federated::optimizer::cached_plan_builder::{
    CachedPlanInputs, build_for_search_cached_with_collection,
};
use proximadb::query::federated::optimizer::plan_builder::PlanBuilderInputs;
use proximadb::query::federated::optimizer::selectivity::FieldStatistics;
use proximadb::query::federated::optimizer::{
    Predicate, PredicateOp, PredicateSelectivityPolicy, PredicateValue,
};

fn record(tier: Tier, scan: Option<f64>, ef: Option<u32>, fresh: Option<u32>) -> TenantTierRecord {
    TenantTierRecord {
        tenant_id: "tenant-a".into(),
        tier,
        scan_budget_gb_hard: scan,
        ef_search_cap: ef,
        freshness_sla_seconds: fresh,
        feature_flags: FeatureFlags::default(),
    }
}

fn predicate(col: &str, val: &str) -> Predicate {
    Predicate {
        column: col.into(),
        op: PredicateOp::Eq,
        value: PredicateValue::String(val.into()),
    }
}

#[tokio::test]
async fn upgrade_emits_event_and_invalidates_warm_cache() {
    // Stage 1: warm the planner cache for tenant-a on collection 'kb'
    // against the community tier.
    let cache = Arc::new(PlanCache::default());
    let before = record(Tier::Tier2, None, None, None);
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let preds = vec![predicate("tier", "community")];
    let inputs_before = CachedPlanInputs {
        plan_inputs: PlanBuilderInputs {
            predicates: &preds,
            field_stats: &stats,
            policy: &policy,
            gls_samples: &[],
            dim: 768,
            recall_target: 0.9,
            collection_gb: 0.1,
            tier: &before,
        },
        corpus_version: 1,
    };
    let _ = build_for_search_cached_with_collection(&cache, &inputs_before, "kb").await;
    let hit = build_for_search_cached_with_collection(&cache, &inputs_before, "kb").await;
    assert!(hit.cache_hit, "cache should be warm");

    // Stage 2: tier transitions to business.
    let after = record(Tier::Tier4, None, None, None);
    let event = detect_transition(&before, &after);
    assert_eq!(event.class, TransitionClass::Upgrade);
    // 2026-Q2 tier rename: Community → Team. See memory note
    // project_tier_rename_2026_05_22.
    assert_eq!(event.tier_before, "team");
    assert_eq!(event.tier_after, "business");
    // Scan budget axis moves up (team default < business default).
    assert_eq!(event.scan_budget_gb.direction, AxisDirection::Up);

    // Stage 3: flush the caches that held the old budget.
    let coord = CacheInvalidationCoordinator::empty()
        .with_plan_cache(cache.clone());
    let summary = coord
        .invalidate_collection("tenant-a", "kb", &[])
        .await;
    assert!(summary.plan_cache_entries >= 1, "should have dropped at least one entry");

    // Stage 4: next planner call (still against community-shape inputs)
    // misses because the cache was flushed.
    let after_inputs = CachedPlanInputs {
        plan_inputs: PlanBuilderInputs {
            predicates: &preds,
            field_stats: &stats,
            policy: &policy,
            gls_samples: &[],
            dim: 768,
            recall_target: 0.9,
            collection_gb: 0.1,
            tier: &after,
        },
        corpus_version: 1,
    };
    let post = build_for_search_cached_with_collection(&cache, &after_inputs, "kb").await;
    assert!(!post.cache_hit, "post-invalidation call must miss");
}

#[tokio::test]
async fn downgrade_emits_event_and_invalidates_too() {
    let cache = Arc::new(PlanCache::default());
    let before = record(Tier::Tier5, None, None, None);
    let after = record(Tier::Tier4, None, None, None);
    let event = detect_transition(&before, &after);
    assert_eq!(event.class, TransitionClass::Downgrade);

    // Pre-populate a cache entry the coordinator should drop.
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let preds = vec![predicate("plan", "enterprise")];
    let inputs = CachedPlanInputs {
        plan_inputs: PlanBuilderInputs {
            predicates: &preds,
            field_stats: &stats,
            policy: &policy,
            gls_samples: &[],
            dim: 1024,
            recall_target: 0.95,
            collection_gb: 1.0,
            tier: &before,
        },
        corpus_version: 1,
    };
    let _ = build_for_search_cached_with_collection(&cache, &inputs, "kb").await;

    let coord = CacheInvalidationCoordinator::empty()
        .with_plan_cache(cache.clone());
    let summary = coord
        .invalidate_collection("tenant-a", "kb", &[])
        .await;
    assert_eq!(summary.plan_cache_entries, 1);
}

#[tokio::test]
async fn no_change_emits_event_but_invalidation_is_an_optional_step() {
    // Application logic decides whether to invalidate on a NoChange
    // event. This test pins the contract: the transition detector
    // produces a NoChange + the invalidator is independent — the
    // caller may run it anyway and the cache reports zero drops.
    let cache = Arc::new(PlanCache::default());
    let before = record(Tier::Tier4, None, None, None);
    let after = record(Tier::Tier4, None, None, None);
    let event = detect_transition(&before, &after);
    assert_eq!(event.class, TransitionClass::NoChange);

    // Warm up a cache entry.
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let preds = vec![predicate("tier", "business")];
    let inputs = CachedPlanInputs {
        plan_inputs: PlanBuilderInputs {
            predicates: &preds,
            field_stats: &stats,
            policy: &policy,
            gls_samples: &[],
            dim: 768,
            recall_target: 0.9,
            collection_gb: 0.1,
            tier: &before,
        },
        corpus_version: 1,
    };
    let _ = build_for_search_cached_with_collection(&cache, &inputs, "kb").await;

    // Even if the caller invalidates anyway (defensive), the cache
    // surfaces what happened — one entry dropped because the
    // coordinator doesn't ask whether the event was NoChange; that's
    // a caller-side decision.
    let coord = CacheInvalidationCoordinator::empty()
        .with_plan_cache(cache.clone());
    let summary = coord
        .invalidate_collection("tenant-a", "kb", &[])
        .await;
    assert_eq!(summary.plan_cache_entries, 1);
}

#[tokio::test]
async fn cross_tenant_transition_does_not_flush_other_tenants() {
    // Tenant-a transitions; tenant-b's cache entries must survive.
    let cache = Arc::new(PlanCache::default());
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let preds = vec![predicate("plan", "community")];
    let tier_a = record(Tier::Tier2, None, None, None);
    let mut tier_b = record(Tier::Tier2, None, None, None);
    tier_b.tenant_id = "tenant-b".into();

    // Warm cache for both tenants.
    for tier in [&tier_a, &tier_b] {
        let inputs = CachedPlanInputs {
            plan_inputs: PlanBuilderInputs {
                predicates: &preds,
                field_stats: &stats,
                policy: &policy,
                gls_samples: &[],
                dim: 768,
                recall_target: 0.9,
                collection_gb: 0.1,
                tier,
            },
            corpus_version: 1,
        };
        let _ = build_for_search_cached_with_collection(&cache, &inputs, "kb").await;
    }

    // Transition is for tenant-a only.
    let after = record(Tier::Tier4, None, None, None);
    let _event = detect_transition(&tier_a, &after);

    // Invalidate only tenant-a's entries.
    let coord = CacheInvalidationCoordinator::empty()
        .with_plan_cache(cache.clone());
    let summary = coord
        .invalidate_collection("tenant-a", "kb", &[])
        .await;
    // Only one entry dropped (the tenant-a one); tenant-b survives.
    assert_eq!(summary.plan_cache_entries, 1);

    // Verify tenant-b still hits.
    let inputs_b = CachedPlanInputs {
        plan_inputs: PlanBuilderInputs {
            predicates: &preds,
            field_stats: &stats,
            policy: &policy,
            gls_samples: &[],
            dim: 768,
            recall_target: 0.9,
            collection_gb: 0.1,
            tier: &tier_b,
        },
        corpus_version: 1,
    };
    let r = build_for_search_cached_with_collection(&cache, &inputs_b, "kb").await;
    assert!(r.cache_hit, "tenant-b's cache survived the tenant-a invalidation");
}

#[tokio::test]
async fn freshness_sla_change_classified_correctly_in_event() {
    // 60s → 15s SLA is faster = upgrade direction per the LLD
    // semantics encoded in tier_transition.
    let before = record(Tier::Tier4, None, None, Some(60));
    let after = record(Tier::Tier4, None, None, Some(15));
    let event = detect_transition(&before, &after);
    assert_eq!(event.class, TransitionClass::Upgrade);
    assert_eq!(event.freshness_sla_seconds.direction, AxisDirection::Up);
}
