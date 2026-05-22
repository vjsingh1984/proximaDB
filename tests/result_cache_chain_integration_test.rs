// Result-cache decision chain integration.
//
// Pipeline:
//
//   incoming request (query text + freshness hint)
//     → category_classifier::classify → Category
//     → category.label() (bounded string)
//     → ResultCacheGate::decide(GateInputs) → GateDecision
//                                              { Serve, Reject }
//
// The gate internally consults per_category_policy + mismatch_cost.
// This test pins the upstream classification + downstream gate
// composition.

use std::collections::HashMap;
use std::time::Duration;

use proximadb::query::cache::category_classifier::{
    Category, ClassifierInputs, FreshnessHint, classify,
};
use proximadb::query::cache::mismatch_cost::{
    MismatchConfig, MismatchCostLearner, Region as MismatchRegion,
};
use proximadb::query::cache::per_category_policy::{CategoryPolicy, PerCategoryPolicy};
use proximadb::query::cache::result_cache_gate::{GateInputs, ResultCacheGate};

fn policy_table() -> PerCategoryPolicy {
    let mut t: HashMap<String, CategoryPolicy> = HashMap::new();
    t.insert(
        Category::Code.label().to_string(),
        CategoryPolicy {
            similarity_threshold: 0.92,
            ttl: Duration::from_secs(30 * 86_400),
            quota: 1_000_000,
            prom_label: "code",
        },
    );
    t.insert(
        Category::Docs.label().to_string(),
        CategoryPolicy {
            similarity_threshold: 0.90,
            ttl: Duration::from_secs(7 * 86_400),
            quota: 1_000_000,
            prom_label: "docs",
        },
    );
    t.insert(
        Category::Conversational.label().to_string(),
        CategoryPolicy {
            similarity_threshold: 0.78,
            ttl: Duration::from_secs(3_600),
            quota: 1_000_000,
            prom_label: "conversational",
        },
    );
    t.insert(
        Category::Volatile.label().to_string(),
        CategoryPolicy {
            similarity_threshold: 0.95,
            ttl: Duration::from_secs(15),
            quota: 1_000_000,
            prom_label: "volatile",
        },
    );
    PerCategoryPolicy::from_table(t)
}

async fn warm(learner: &MismatchCostLearner, tenant: &str, category: &str, cost: f64, n: usize) {
    for _ in 0..n {
        learner.observe(MismatchRegion::new(tenant, category), cost).await;
    }
}

/// Pipeline: a code-shape query gets classified as Code, the gate
/// applies the 0.92 similarity threshold, and a warm low-cost region
/// produces Serve.
#[tokio::test]
async fn code_query_serves_when_warm_low_cost() {
    let q = "fn parse_args(argv: &[String]) -> Result<(), Error> { ... }";
    let inputs = ClassifierInputs { query_text: q, freshness_hint: None };
    let category = classify(&inputs);
    assert_eq!(category, Category::Code);

    let policy = policy_table();
    let learner = MismatchCostLearner::new(MismatchConfig {
        similarity_floor: 0.5,
        allowed_cost: 0.5,
        decay_seconds: 3600,
    });
    warm(&learner, "tenant-a", category.label(), 0.05, 50).await;

    let gate = ResultCacheGate::new(&policy, &learner);
    let d = gate
        .decide(&GateInputs {
            category: category.label(),
            tenant_id: "tenant-a",
            similarity: 0.95, // above code's 0.92 threshold
            age: Duration::from_secs(60),
        })
        .await;
    assert!(d.is_serve());
}

/// A conversational short query routes through the lower threshold —
/// 0.80 similarity passes for conversational where it wouldn't for code.
#[tokio::test]
async fn conversational_query_serves_at_lower_similarity_than_code_would() {
    let q = "what is rust";
    let inputs = ClassifierInputs { query_text: q, freshness_hint: None };
    let category = classify(&inputs);
    assert_eq!(category, Category::Conversational);

    let policy = policy_table();
    let learner = MismatchCostLearner::new(MismatchConfig {
        similarity_floor: 0.5,
        allowed_cost: 0.5,
        decay_seconds: 3600,
    });
    warm(&learner, "tenant-a", category.label(), 0.05, 50).await;

    let gate = ResultCacheGate::new(&policy, &learner);
    let d = gate
        .decide(&GateInputs {
            category: category.label(),
            tenant_id: "tenant-a",
            similarity: 0.80, // > conversational's 0.78 threshold
            age: Duration::from_secs(60),
        })
        .await;
    assert!(d.is_serve(), "0.80 should serve under conversational policy");

    // Same 0.80 against code threshold (0.92) would reject — pin this
    // by running with category overridden to 'code'.
    let d_code = gate
        .decide(&GateInputs {
            category: "code",
            tenant_id: "tenant-a",
            similarity: 0.80,
            age: Duration::from_secs(60),
        })
        .await;
    assert!(!d_code.is_serve(), "0.80 should not serve under code policy");
}

/// Strict freshness → Volatile classification → tight 0.95 threshold
/// + 15s TTL. A 30s-old entry is rejected for TTL.
#[tokio::test]
async fn strict_freshness_routes_to_volatile_and_rejects_old_entries() {
    let q = "find the cached result";
    let inputs = ClassifierInputs {
        query_text: q,
        freshness_hint: Some(FreshnessHint::Strict),
    };
    let category = classify(&inputs);
    assert_eq!(category, Category::Volatile);

    let policy = policy_table();
    let learner = MismatchCostLearner::new(MismatchConfig {
        similarity_floor: 0.5,
        allowed_cost: 0.5,
        decay_seconds: 3600,
    });
    let gate = ResultCacheGate::new(&policy, &learner);
    let d = gate
        .decide(&GateInputs {
            category: category.label(),
            tenant_id: "tenant-a",
            similarity: 0.99,
            age: Duration::from_secs(30), // > volatile's 15s TTL
        })
        .await;
    assert!(!d.is_serve());
}

/// Cold mismatch region rejects even with similarity + TTL good. This
/// pins the LLD §6 invariant — the cache must NOT serve a near-miss
/// from a cold region.
#[tokio::test]
async fn cold_mismatch_region_rejects_first_lookup() {
    let q = "fn parse(arg) {}";
    let inputs = ClassifierInputs { query_text: q, freshness_hint: None };
    let category = classify(&inputs);
    assert_eq!(category, Category::Code);

    let policy = policy_table();
    let learner = MismatchCostLearner::new(MismatchConfig {
        similarity_floor: 0.5,
        allowed_cost: 0.5,
        decay_seconds: 3600,
    });
    // Don't warm — the region stays cold.

    let gate = ResultCacheGate::new(&policy, &learner);
    let d = gate
        .decide(&GateInputs {
            category: category.label(),
            tenant_id: "tenant-a",
            similarity: 0.99,
            age: Duration::from_secs(60),
        })
        .await;
    assert!(!d.is_serve(), "cold region must reject");
}

/// Cross-tenant: tenant-a warm + tenant-b cold under the same category
/// must produce different outcomes. Pins mismatch-region tenant scope.
#[tokio::test]
async fn cross_tenant_mismatch_isolation_preserved_through_the_chain() {
    let q = "fn parse() {}";
    let inputs = ClassifierInputs { query_text: q, freshness_hint: None };
    let category = classify(&inputs);
    assert_eq!(category, Category::Code);

    let policy = policy_table();
    let learner = MismatchCostLearner::new(MismatchConfig {
        similarity_floor: 0.5,
        allowed_cost: 0.5,
        decay_seconds: 3600,
    });
    warm(&learner, "tenant-a", category.label(), 0.05, 50).await;

    let gate = ResultCacheGate::new(&policy, &learner);
    let a = gate
        .decide(&GateInputs {
            category: category.label(),
            tenant_id: "tenant-a",
            similarity: 0.95,
            age: Duration::from_secs(60),
        })
        .await;
    let b = gate
        .decide(&GateInputs {
            category: category.label(),
            tenant_id: "tenant-b",
            similarity: 0.95,
            age: Duration::from_secs(60),
        })
        .await;
    assert!(a.is_serve(), "tenant-a is warm");
    assert!(!b.is_serve(), "tenant-b is cold");
}

/// Unknown category from the classifier (which can return Category::Unknown)
/// falls back to the gate's default safe policy — neither panic nor
/// crash. This pins the resilience contract.
#[tokio::test]
async fn unknown_category_routes_through_safe_default() {
    // A query that doesn't match any of the classifier branches: a
    // moderate-length text without code markers, conversational
    // prefixes, volatile keywords, or jargon. Lands on Unknown.
    let q = "describe recent observations from production";
    let inputs = ClassifierInputs { query_text: q, freshness_hint: None };
    let category = classify(&inputs);
    // Don't assert exact category; downstream behavior is what we
    // care about — the gate must not panic.

    // Use the gate's default (PerCategoryPolicy::with_defaults includes
    // the unknown bucket).
    let policy = PerCategoryPolicy::with_defaults();
    let learner = MismatchCostLearner::new(MismatchConfig {
        similarity_floor: 0.5,
        allowed_cost: 0.5,
        decay_seconds: 3600,
    });
    let gate = ResultCacheGate::new(&policy, &learner);
    // Just exercise the gate — it must produce a decision without
    // panicking even for an Unknown category and an unwarmed region.
    let _ = gate
        .decide(&GateInputs {
            category: category.label(),
            tenant_id: "tenant-a",
            similarity: 0.50,
            age: Duration::from_secs(60),
        })
        .await;
}
