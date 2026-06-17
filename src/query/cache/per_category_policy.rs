// Per-category cache policy — arXiv 2510.26835.
//
// Production semantic-cache workloads aren't homogeneous:
//
//   - Code queries cluster densely in embedding space (40-60% hit rate).
//     A high similarity threshold (0.92+) is safe — distinct queries are
//     reliably distinguishable.
//
//   - Conversational queries are sparse (5-15% hit rate). A lower threshold
//     (0.78) is required to ever hit; matching at 0.92 would never trigger.
//
//   - Volatile content (stock data, headlines) needs short TTLs measured in
//     seconds. Stable content (code patterns, runbook entries) can be cached
//     for months.
//
// A single global threshold + TTL is therefore provably wrong for any
// production deployment. This module exposes a policy table keyed on a
// category tag the gateway attaches to each request.
//
// Per LLD §6.2 the tag is supplied by the gateway based on the inbound query
// shape (the tenant can override). Unknown categories fall back to a safe
// default (low threshold, short TTL, small quota) so a typo can't poison
// the cache.

use std::collections::HashMap;
use std::time::Duration;

/// Per-category cache parameters. Cloneable so the planner can hand them
/// to the underlying cache adapter without taking a reference.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CategoryPolicy {
    /// Minimum cosine similarity for a cache hit to be considered.
    pub similarity_threshold: f64,
    /// How long the entry stays valid before re-execution is forced.
    pub ttl: Duration,
    /// Cap on entries the cache holds for this category. 0 = unbounded.
    pub quota: u32,
    /// Bounded Prometheus label — matches the category name to keep
    /// cardinality safe.
    pub prom_label: &'static str,
}

impl CategoryPolicy {
    /// Safe fallback applied to unknown categories. Low threshold means
    /// caching is unlikely (the category is treated as sparse), short TTL
    /// means even if we cache, we re-validate quickly.
    pub const fn fallback() -> Self {
        Self {
            similarity_threshold: 0.78,
            ttl: Duration::from_secs(60),
            quota: 1_000,
            prom_label: "unknown",
        }
    }
}

/// Default category table. Values are anchored on the production
/// percentages reported in 2510.26835 §3, conservatively rounded toward
/// "fewer false hits" since false hits are the dominant tenant-visible
/// failure mode.
pub fn default_table() -> HashMap<&'static str, CategoryPolicy> {
    let mut t = HashMap::new();
    t.insert(
        "code",
        CategoryPolicy {
            similarity_threshold: 0.92,
            ttl: Duration::from_secs(30 * 24 * 3600),
            quota: 100_000,
            prom_label: "code",
        },
    );
    t.insert(
        "docs",
        CategoryPolicy {
            similarity_threshold: 0.90,
            ttl: Duration::from_secs(7 * 24 * 3600),
            quota: 50_000,
            prom_label: "docs",
        },
    );
    t.insert(
        "conversational",
        CategoryPolicy {
            similarity_threshold: 0.78,
            ttl: Duration::from_secs(60 * 60),
            quota: 20_000,
            prom_label: "conversational",
        },
    );
    t.insert(
        "specialized",
        CategoryPolicy {
            similarity_threshold: 0.82,
            ttl: Duration::from_secs(15 * 60),
            quota: 5_000,
            prom_label: "specialized",
        },
    );
    t.insert(
        "volatile",
        CategoryPolicy {
            similarity_threshold: 0.95,
            ttl: Duration::from_secs(15),
            quota: 1_000,
            prom_label: "volatile",
        },
    );
    t
}

/// Categories the gateway can attach to a request. Tenants override the
/// per-category numbers but the **category names** stay bounded — they
/// double as Prometheus labels so cardinality is fixed.
pub struct PerCategoryPolicy {
    table: HashMap<String, CategoryPolicy>,
    fallback: CategoryPolicy,
}

impl PerCategoryPolicy {
    /// Build a policy from the default table.
    pub fn with_defaults() -> Self {
        let table = default_table()
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
        Self {
            table,
            fallback: CategoryPolicy::fallback(),
        }
    }

    /// Build a policy from a custom table — used by tenant overrides.
    pub fn from_table(table: HashMap<String, CategoryPolicy>) -> Self {
        Self {
            table,
            fallback: CategoryPolicy::fallback(),
        }
    }

    /// Look up the policy for a category. Unknown categories return the
    /// safe fallback (low threshold, short TTL, small quota).
    pub fn lookup(&self, category: &str) -> CategoryPolicy {
        self.table.get(category).copied().unwrap_or(self.fallback)
    }

    /// Override one entry. Useful for per-tenant tweaks read from the tier
    /// store at startup.
    pub fn set(&mut self, category: impl Into<String>, policy: CategoryPolicy) {
        self.table.insert(category.into(), policy);
    }

    /// Total number of categories defined in the table (for observability).
    pub fn len(&self) -> usize {
        self.table.len()
    }

    /// Whether the table is empty — used by the policy validator to detect
    /// misconfiguration at startup before traffic arrives.
    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_include_documented_categories() {
        let p = PerCategoryPolicy::with_defaults();
        // The five categories anchored on 2510.26835.
        for name in ["code", "docs", "conversational", "specialized", "volatile"] {
            let pol = p.lookup(name);
            assert!(pol.similarity_threshold > 0.0 && pol.similarity_threshold <= 1.0);
            assert!(pol.ttl > Duration::ZERO);
        }
    }

    #[test]
    fn code_has_high_threshold_and_long_ttl() {
        let p = PerCategoryPolicy::with_defaults();
        let code = p.lookup("code");
        assert!(
            code.similarity_threshold >= 0.9,
            "code threshold {} too low",
            code.similarity_threshold
        );
        assert!(
            code.ttl >= Duration::from_secs(7 * 24 * 3600),
            "code TTL too short"
        );
    }

    #[test]
    fn conversational_has_low_threshold_and_short_ttl() {
        let p = PerCategoryPolicy::with_defaults();
        let convo = p.lookup("conversational");
        assert!(
            convo.similarity_threshold <= 0.80,
            "conversational threshold {} too high",
            convo.similarity_threshold
        );
        assert!(
            convo.ttl <= Duration::from_secs(6 * 3600),
            "conversational TTL too long"
        );
    }

    #[test]
    fn volatile_has_very_short_ttl_and_strict_threshold() {
        let p = PerCategoryPolicy::with_defaults();
        let v = p.lookup("volatile");
        assert!(v.ttl <= Duration::from_secs(60));
        assert!(v.similarity_threshold >= 0.93);
    }

    #[test]
    fn unknown_category_falls_back() {
        let p = PerCategoryPolicy::with_defaults();
        let unk = p.lookup("definitely-not-a-known-category");
        assert_eq!(unk.prom_label, "unknown");
        assert_eq!(unk, CategoryPolicy::fallback());
    }

    #[test]
    fn tenant_override_replaces_defaults() {
        let mut p = PerCategoryPolicy::with_defaults();
        let custom = CategoryPolicy {
            similarity_threshold: 0.99,
            ttl: Duration::from_secs(1),
            quota: 10,
            prom_label: "code",
        };
        p.set("code", custom);
        assert_eq!(p.lookup("code"), custom);
    }

    #[test]
    fn empty_policy_is_empty() {
        let p = PerCategoryPolicy::from_table(HashMap::new());
        assert!(p.is_empty());
        assert_eq!(p.len(), 0);
    }

    #[test]
    fn prom_labels_are_static_strings() {
        // Bounded-cardinality invariant — the label must be a `&'static str`
        // so Prometheus metric registration can use it without allocation.
        let p = PerCategoryPolicy::with_defaults();
        let labels: Vec<&'static str> =
            ["code", "docs", "conversational", "specialized", "volatile"]
                .iter()
                .map(|name| p.lookup(name).prom_label)
                .collect();
        assert_eq!(
            labels,
            vec!["code", "docs", "conversational", "specialized", "volatile"]
        );
    }
}
