// Cached planner integration — exercises the cache loop end-to-end.
//
// PlanCache + cached_plan_builder + invalidation_coordinator are each
// unit-tested in isolation. This test proves the composition:
//
//   request → cached_plan_builder::build_for_search_cached_with_collection
//     → first call: miss + populates
//     → second call (identical shape): hit
//     → schema bump via invalidation_coordinator::invalidate_collection
//     → next call: miss again
//
// And on the cross-tenant axis: same shape under different tenants
// must produce two distinct cache entries even with identical
// predicate digests.

use std::sync::Arc;

use proximadb::catalog::tenant_tier::TenantTierRecord;
use proximadb::query::cache::batch_group::BatchGroupCache;
use proximadb::query::cache::invalidation_coordinator::CacheInvalidationCoordinator;
use proximadb::query::cache::plan_cache::PlanCache;
use proximadb::query::federated::optimizer::cached_plan_builder::{
    CachedPlanInputs, build_for_search_cached_with_collection,
};
use proximadb::query::federated::optimizer::plan_builder::PlanBuilderInputs;
use proximadb::query::federated::optimizer::selectivity::FieldStatistics;
use proximadb::query::federated::optimizer::{Predicate, PredicateOp, PredicateSelectivityPolicy, PredicateValue};

fn predicate(col: &str, val: &str) -> Predicate {
    Predicate {
        column: col.into(),
        op: PredicateOp::Eq,
        value: PredicateValue::String(val.into()),
    }
}

#[tokio::test]
async fn first_call_misses_second_call_hits() {
    let cache = PlanCache::default();
    let tier = TenantTierRecord::fail_safe("tenant-a");
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let preds = vec![predicate("tier", "free")];

    let inputs = CachedPlanInputs {
        plan_inputs: PlanBuilderInputs {
            predicates: &preds,
            field_stats: &stats,
            policy: &policy,
            gls_samples: &[],
            dim: 768,
            recall_target: 0.9,
            collection_gb: 0.1,
            tier: &tier,
        },
        corpus_version: 1,
    };

    let first = build_for_search_cached_with_collection(&cache, &inputs, "kb").await;
    assert!(!first.cache_hit, "first call must miss");

    let second = build_for_search_cached_with_collection(&cache, &inputs, "kb").await;
    assert!(second.cache_hit, "second identical call must hit");
    assert_eq!(first.plan, second.plan, "cached plan must match fresh plan");
}

#[tokio::test]
async fn schema_bump_via_coordinator_invalidates_cache() {
    let cache = Arc::new(PlanCache::default());
    let tier = TenantTierRecord::fail_safe("tenant-a");
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let preds = vec![predicate("tier", "free")];
    let inputs = CachedPlanInputs {
        plan_inputs: PlanBuilderInputs {
            predicates: &preds,
            field_stats: &stats,
            policy: &policy,
            gls_samples: &[],
            dim: 768,
            recall_target: 0.9,
            collection_gb: 0.1,
            tier: &tier,
        },
        corpus_version: 1,
    };
    let _ = build_for_search_cached_with_collection(&cache, &inputs, "kb").await;
    let hit = build_for_search_cached_with_collection(&cache, &inputs, "kb").await;
    assert!(hit.cache_hit, "warm-up second call should hit");

    // Invalidate via the coordinator (the production path the gateway uses).
    let coord = CacheInvalidationCoordinator::empty()
        .with_plan_cache(cache.clone());
    let summary = coord
        .invalidate_collection("tenant-a", "kb", &[])
        .await;
    assert_eq!(
        summary.plan_cache_entries, 1,
        "coordinator should report one dropped entry"
    );

    let after = build_for_search_cached_with_collection(&cache, &inputs, "kb").await;
    assert!(
        !after.cache_hit,
        "post-invalidation call must miss again"
    );
}

#[tokio::test]
async fn cross_tenant_traces_keep_distinct_cache_entries() {
    let cache = PlanCache::default();
    let tier_a = TenantTierRecord::fail_safe("tenant-a");
    let tier_b = TenantTierRecord::fail_safe("tenant-b");
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let preds = vec![predicate("tier", "free")];

    let inputs_a = CachedPlanInputs {
        plan_inputs: PlanBuilderInputs {
            predicates: &preds,
            field_stats: &stats,
            policy: &policy,
            gls_samples: &[],
            dim: 768,
            recall_target: 0.9,
            collection_gb: 0.1,
            tier: &tier_a,
        },
        corpus_version: 1,
    };
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

    let ra = build_for_search_cached_with_collection(&cache, &inputs_a, "kb").await;
    assert!(!ra.cache_hit);

    // Same shape under a different tenant must miss — the cache is
    // tenant-scoped.
    let rb = build_for_search_cached_with_collection(&cache, &inputs_b, "kb").await;
    assert!(!rb.cache_hit, "tenant-b must miss even though tenant-a populated");
}

#[tokio::test]
async fn different_collections_dont_share_cache_entries() {
    let cache = PlanCache::default();
    let tier = TenantTierRecord::fail_safe("tenant-a");
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let preds = vec![predicate("tier", "free")];
    let inputs = CachedPlanInputs {
        plan_inputs: PlanBuilderInputs {
            predicates: &preds,
            field_stats: &stats,
            policy: &policy,
            gls_samples: &[],
            dim: 768,
            recall_target: 0.9,
            collection_gb: 0.1,
            tier: &tier,
        },
        corpus_version: 1,
    };
    let _ = build_for_search_cached_with_collection(&cache, &inputs, "kb-1").await;
    let r = build_for_search_cached_with_collection(&cache, &inputs, "kb-2").await;
    assert!(
        !r.cache_hit,
        "same shape on a different collection must miss"
    );
}

#[tokio::test]
async fn invalidation_summary_total_reflects_cache_contribution() {
    let plan_cache = Arc::new(PlanCache::default());
    let batch_group = Arc::new(BatchGroupCache::default());
    let tier = TenantTierRecord::fail_safe("tenant-a");
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let preds = vec![predicate("tier", "free")];
    let inputs = CachedPlanInputs {
        plan_inputs: PlanBuilderInputs {
            predicates: &preds,
            field_stats: &stats,
            policy: &policy,
            gls_samples: &[],
            dim: 768,
            recall_target: 0.9,
            collection_gb: 0.1,
            tier: &tier,
        },
        corpus_version: 1,
    };
    // Populate plan_cache with two distinct keys (different recall
    // targets → distinct cache entries even with the same predicates).
    let _ = build_for_search_cached_with_collection(&plan_cache, &inputs, "kb").await;
    let mut inputs2 = CachedPlanInputs {
        plan_inputs: PlanBuilderInputs {
            predicates: &preds,
            field_stats: &stats,
            policy: &policy,
            gls_samples: &[],
            dim: 768,
            recall_target: 0.95,
            collection_gb: 0.1,
            tier: &tier,
        },
        corpus_version: 1,
    };
    let _ = build_for_search_cached_with_collection(&plan_cache, &inputs2, "kb").await;
    let _ = inputs2; // keep alive; silences any unused-write lint

    let coord = CacheInvalidationCoordinator::empty()
        .with_plan_cache(plan_cache)
        .with_batch_group(batch_group);
    let summary = coord
        .invalidate_collection("tenant-a", "kb", &[])
        .await;
    // Two plan-cache entries; no batch groups attached to this
    // collection in the test.
    assert_eq!(summary.plan_cache_entries, 2);
    assert_eq!(summary.batch_groups_closed, 0);
    assert_eq!(summary.total(), 2);
}
