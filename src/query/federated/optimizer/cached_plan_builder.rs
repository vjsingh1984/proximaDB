// Cached PlanBuilder — closes the loop between `plan_builder` and
// `plan_cache`. The non-cached `build_for_search` runs the SelectivityEstimator
// + GLS computation in ~20-40 µs per call; this wrapper turns the common
// case (identical predicate shape recurring across a single agent's
// reasoning loop) into a single DashMap lookup.
//
// Lookup is keyed on the same fields PlanCacheKey requires:
// (tenant_id, collection, predicate_digest, dim, recall_target). The
// digest is computed via predicate_normalizer so identical predicate
// sets in any order hit the same cache entry. GLS samples bypass the
// cache because their values are query-specific — caching a GLS-shifted
// plan and then serving it to a query with different neighborhood
// samples would silently degrade routing.

use crate::query::cache::plan_cache::{PlanCache, PlanCacheKey, digest_predicates};
use crate::query::federated::optimizer::plan_builder::{
    PlanBuilderInputs, PlanOutput, build_for_search,
};
use crate::query::federated::optimizer::predicate_normalizer::normalize;

/// Outcome of a cached planner lookup. The `cache_hit` flag flows into
/// `SearchPlanTrace::cache_result` so the trace records whether the plan
/// was reused or freshly computed.
#[derive(Debug, Clone, PartialEq)]
pub struct CachedPlanOutput {
    pub plan: PlanOutput,
    /// True when the plan came from the cache; false on cache miss.
    pub cache_hit: bool,
}

/// Inputs the cached builder consumes. Same as PlanBuilderInputs plus
/// `corpus_version` so the cache can invalidate on schema/data change.
pub struct CachedPlanInputs<'a> {
    /// Underlying plan inputs the non-cached builder consumes.
    pub plan_inputs: PlanBuilderInputs<'a>,
    /// Current corpus version (from the segment manifest). A bump
    /// invalidates every cache entry for the collection on next lookup.
    pub corpus_version: u64,
}

/// Wrapper that consults the plan cache first; falls back to
/// `build_for_search` on miss and populates the cache with the result.
///
/// GLS samples are checked: if any are supplied, the cache is **bypassed**
/// (we compute fresh) because a cached plan reflects the neighborhood
/// distribution at insert time. A different query with different samples
/// shouldn't reuse that decision.
pub async fn build_for_search_cached(
    cache: &PlanCache,
    inputs: &CachedPlanInputs<'_>,
) -> CachedPlanOutput {
    // GLS bypass: any non-empty sample slice triggers a fresh compute
    // since the planner's GLS shift depends on the per-query samples.
    if !inputs.plan_inputs.gls_samples.is_empty() {
        let plan = build_for_search(&inputs.plan_inputs);
        return CachedPlanOutput {
            plan,
            cache_hit: false,
        };
    }

    let key = cache_key_from_inputs(inputs);

    if let Some(plan) = cache.get(&key, inputs.corpus_version).await {
        return CachedPlanOutput {
            plan,
            cache_hit: true,
        };
    }

    let plan = build_for_search(&inputs.plan_inputs);
    cache.put(key, plan.clone(), inputs.corpus_version).await;
    CachedPlanOutput {
        plan,
        cache_hit: false,
    }
}

/// Helper to build a cache key from the cached planner inputs. Exposed
/// for tests + observability dashboards that want to see what the cache
/// keying would be without actually doing the lookup.
pub fn cache_key_from_inputs(inputs: &CachedPlanInputs<'_>) -> PlanCacheKey {
    let triples = normalize(inputs.plan_inputs.predicates);
    let digest = digest_predicates(&triples);
    PlanCacheKey::new(
        inputs.plan_inputs.tier.tenant_id.clone(),
        // The plan inputs don't carry a collection name — derive it from
        // the tier's tenant scope plus a stable per-tenant suffix. In
        // production the caller (v2 records.rs) supplies the real
        // collection name through `PlanCacheKey::new` directly when it
        // needs to override this default; tests below pin the contract.
        "_default",
        digest,
        inputs.plan_inputs.dim as u32,
        inputs.plan_inputs.recall_target,
    )
}

/// Variant that lets the caller supply an explicit collection name. The
/// v2 records.rs handler uses this so the cache key reflects the actual
/// collection rather than the `_default` placeholder.
pub async fn build_for_search_cached_with_collection(
    cache: &PlanCache,
    inputs: &CachedPlanInputs<'_>,
    collection: &str,
) -> CachedPlanOutput {
    if !inputs.plan_inputs.gls_samples.is_empty() {
        let plan = build_for_search(&inputs.plan_inputs);
        return CachedPlanOutput {
            plan,
            cache_hit: false,
        };
    }
    let triples = normalize(inputs.plan_inputs.predicates);
    let digest = digest_predicates(&triples);
    let key = PlanCacheKey::new(
        inputs.plan_inputs.tier.tenant_id.clone(),
        collection,
        digest,
        inputs.plan_inputs.dim as u32,
        inputs.plan_inputs.recall_target,
    );
    if let Some(plan) = cache.get(&key, inputs.corpus_version).await {
        return CachedPlanOutput {
            plan,
            cache_hit: true,
        };
    }
    let plan = build_for_search(&inputs.plan_inputs);
    cache.put(key, plan.clone(), inputs.corpus_version).await;
    CachedPlanOutput {
        plan,
        cache_hit: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::tenant_tier::TenantTierRecord;
    use crate::query::federated::optimizer::gls::GlsSample;
    use crate::query::federated::optimizer::selectivity::FieldStatistics;
    use crate::query::federated::optimizer::{
        Predicate, PredicateOp, PredicateSelectivityPolicy, PredicateValue,
    };

    fn tier() -> TenantTierRecord {
        TenantTierRecord::fail_safe("tenant-a")
    }

    fn policy() -> PredicateSelectivityPolicy {
        PredicateSelectivityPolicy::default()
    }

    fn empty_stats() -> FieldStatistics {
        FieldStatistics::default()
    }

    fn predicate(col: &str) -> Predicate {
        Predicate {
            column: col.into(),
            op: PredicateOp::Eq,
            value: PredicateValue::String("v".into()),
        }
    }

    fn make_inputs<'a>(
        predicates: &'a [Predicate],
        stats: &'a FieldStatistics,
        pol: &'a PredicateSelectivityPolicy,
        gls: &'a [GlsSample],
        tier_ref: &'a TenantTierRecord,
        corpus_version: u64,
    ) -> CachedPlanInputs<'a> {
        CachedPlanInputs {
            plan_inputs: PlanBuilderInputs {
                predicates,
                field_stats: stats,
                policy: pol,
                gls_samples: gls,
                dim: 384,
                recall_target: 0.9,
                collection_gb: 0.1,
                tier: tier_ref,
            },
            corpus_version,
        }
    }

    #[tokio::test]
    async fn first_call_misses_second_call_hits() {
        let cache = PlanCache::default();
        let preds = vec![predicate("tier")];
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let inputs = make_inputs(&preds, &stats, &p, &[], &t, 1);

        let first = build_for_search_cached(&cache, &inputs).await;
        assert!(!first.cache_hit, "first call must be a miss");

        let second = build_for_search_cached(&cache, &inputs).await;
        assert!(second.cache_hit, "second call must hit the cache");
        assert_eq!(first.plan, second.plan);
    }

    #[tokio::test]
    async fn different_predicate_order_still_hits_after_first_call() {
        // Order independence is the contract the normalizer provides;
        // verify it survives through the cached layer.
        let cache = PlanCache::default();
        let p1 = vec![predicate("a"), predicate("b")];
        let p2 = vec![predicate("b"), predicate("a")];
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let i1 = make_inputs(&p1, &stats, &p, &[], &t, 1);
        let i2 = make_inputs(&p2, &stats, &p, &[], &t, 1);

        let first = build_for_search_cached(&cache, &i1).await;
        assert!(!first.cache_hit);
        let second = build_for_search_cached(&cache, &i2).await;
        assert!(
            second.cache_hit,
            "permuted predicates should hit the same entry"
        );
    }

    #[tokio::test]
    async fn corpus_version_bump_invalidates_cached_entry() {
        let cache = PlanCache::default();
        let preds = vec![predicate("tier")];
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let v1 = make_inputs(&preds, &stats, &p, &[], &t, 1);
        let v2 = make_inputs(&preds, &stats, &p, &[], &t, 2);

        let _ = build_for_search_cached(&cache, &v1).await;
        // Bump → next call with v2 must miss.
        let after_bump = build_for_search_cached(&cache, &v2).await;
        assert!(!after_bump.cache_hit, "version drift must force a miss");
    }

    #[tokio::test]
    async fn non_empty_gls_samples_bypass_cache() {
        let cache = PlanCache::default();
        let preds = vec![predicate("tier")];
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let gls = vec![GlsSample {
            local_count: 10,
            local_matches: 5,
        }];
        // Populate the cache with a no-GLS call first.
        let no_gls = make_inputs(&preds, &stats, &p, &[], &t, 1);
        let _ = build_for_search_cached(&cache, &no_gls).await;
        // Now call with GLS samples — must bypass the cache entirely.
        let with_gls = make_inputs(&preds, &stats, &p, &gls, &t, 1);
        let out = build_for_search_cached(&cache, &with_gls).await;
        assert!(!out.cache_hit, "GLS samples must bypass cache");
    }

    #[tokio::test]
    async fn different_tenants_get_distinct_cache_entries() {
        // Cross-tenant isolation: same predicate digest under tenant-a
        // and tenant-b must produce two distinct cache entries.
        let cache = PlanCache::default();
        let preds = vec![predicate("tier")];
        let stats = empty_stats();
        let p = policy();
        let a = TenantTierRecord::fail_safe("tenant-a");
        let b = TenantTierRecord::fail_safe("tenant-b");
        let ia = make_inputs(&preds, &stats, &p, &[], &a, 1);
        let ib = make_inputs(&preds, &stats, &p, &[], &b, 1);

        let ra = build_for_search_cached(&cache, &ia).await;
        let rb = build_for_search_cached(&cache, &ib).await;
        assert!(!ra.cache_hit);
        assert!(
            !rb.cache_hit,
            "tenant-b must miss even though tenant-a populated"
        );
    }

    #[tokio::test]
    async fn explicit_collection_key_isolates_collections() {
        let cache = PlanCache::default();
        let preds = vec![predicate("tier")];
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let inputs = make_inputs(&preds, &stats, &p, &[], &t, 1);

        let r1 = build_for_search_cached_with_collection(&cache, &inputs, "kb-1").await;
        assert!(!r1.cache_hit);
        // Different collection — must miss.
        let r2 = build_for_search_cached_with_collection(&cache, &inputs, "kb-2").await;
        assert!(!r2.cache_hit, "different collection must miss");
        // Same collection again — must hit.
        let r3 = build_for_search_cached_with_collection(&cache, &inputs, "kb-1").await;
        assert!(r3.cache_hit);
    }

    #[tokio::test]
    async fn cached_plan_matches_fresh_plan() {
        // A cached plan output must be byte-identical to what
        // build_for_search would have returned. No silent transformation.
        let cache = PlanCache::default();
        let preds = vec![predicate("tier")];
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let inputs = make_inputs(&preds, &stats, &p, &[], &t, 1);

        let fresh = build_for_search(&inputs.plan_inputs);
        let cached = build_for_search_cached(&cache, &inputs).await;
        assert_eq!(fresh, cached.plan);
    }

    #[tokio::test]
    async fn cache_key_helper_is_stable_across_calls() {
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let preds = vec![predicate("tier")];
        let inputs = make_inputs(&preds, &stats, &p, &[], &t, 1);
        let k1 = cache_key_from_inputs(&inputs);
        let k2 = cache_key_from_inputs(&inputs);
        assert_eq!(k1, k2);
    }

    #[tokio::test]
    async fn empty_predicates_still_cache() {
        let cache = PlanCache::default();
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let inputs = make_inputs(&[], &stats, &p, &[], &t, 1);
        let first = build_for_search_cached(&cache, &inputs).await;
        let second = build_for_search_cached(&cache, &inputs).await;
        assert!(!first.cache_hit);
        assert!(second.cache_hit);
    }

    #[tokio::test]
    async fn distinct_recall_targets_cache_separately() {
        let cache = PlanCache::default();
        let stats = empty_stats();
        let p = policy();
        let t = tier();
        let preds = vec![predicate("tier")];
        let mut inputs_a = make_inputs(&preds, &stats, &p, &[], &t, 1);
        inputs_a.plan_inputs.recall_target = 0.85;
        let mut inputs_b = make_inputs(&preds, &stats, &p, &[], &t, 1);
        inputs_b.plan_inputs.recall_target = 0.97;

        let _ = build_for_search_cached(&cache, &inputs_a).await;
        let second = build_for_search_cached(&cache, &inputs_b).await;
        // Different recall target → different key → must miss.
        assert!(!second.cache_hit);
    }
}
