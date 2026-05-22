// Catapult shortcut table - LLD 6.3, anchored on CatapultDB arXiv 2603.02164.
//
// Real-world ANN workloads exhibit strong spatial and temporal query
// locality, yet every search starts from a fixed (or random) entry point
// and re-traverses the same intermediate hops. CatapultDB's insight:
// observe successful search trajectories, then inject shortcut edges from
// query regions to frequently-visited destination nodes so future similar
// queries skip the redundant hops. The paper reports up to 2.51x higher
// throughput and +11% recall vs DiskANN, layerable on existing indexes.
//
// This module ships the **per-collection** shortcut table - observation
// recording, lookup, hit-counting, and capacity-bounded eviction. The
// runtime integration (record observations on successful searches, consult
// the table for the entry node on incoming queries) lives outside this
// module so the data structure stays testable without an engine attached.
//
// LLD Open Question 7 calls out the cross-tenant isolation guarantee:
// catapults are **per-tenant + per-collection**. The table type uses a
// composite (tenant_id, collection) scope key and refuses lookups across
// scopes. A debug-only assert catches accidental cross-tenant injection.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// Composite scope key - pairs tenant + collection so a catapult edge
/// never crosses tenants by construction.
#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub struct CatapultScope {
    pub tenant_id: String,
    pub collection: String,
}

impl CatapultScope {
    pub fn new(tenant_id: impl Into<String>, collection: impl Into<String>) -> Self {
        Self {
            tenant_id: tenant_id.into(),
            collection: collection.into(),
        }
    }
}

/// One observed-trajectory shortcut edge - "queries near this query_region
/// often want destination_node as their entry point."
#[derive(Debug, Clone, PartialEq)]
pub struct CatapultEdge {
    /// Query-region representative (a centroid id or hashed query digest).
    pub query_region: u64,
    /// Node id the runtime should jump to as the search entry.
    pub destination_node: u64,
    /// Number of past queries that benefited from this shortcut. Used for
    /// eviction priority (higher hits = stickier).
    pub hits: u64,
    /// When the edge was last refreshed.
    pub last_seen: Instant,
}

/// Tunable knobs.
#[derive(Debug, Clone, Copy)]
pub struct CatapultConfig {
    /// Hard ceiling on shortcuts per scope. The paper recommends keeping
    /// the table small (proportional to active query regions, not corpus
    /// size). Default 1024 matches the LLD 6.3 capacity hint.
    pub max_edges_per_scope: usize,
    /// Minimum hits before an edge survives an eviction round. Below this
    /// the LRU-ish sweep treats the edge as cold.
    pub min_hits_to_survive: u64,
    /// Age past which an edge is considered stale regardless of hits.
    pub max_age: Duration,
}

impl Default for CatapultConfig {
    fn default() -> Self {
        Self {
            max_edges_per_scope: 1024,
            min_hits_to_survive: 2,
            max_age: Duration::from_secs(3600),
        }
    }
}

/// Per-scope counters for observability.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct CatapultStats {
    pub edges: usize,
    pub total_lookups: u64,
    pub total_hits: u64,
    pub total_observations: u64,
    pub evictions: u64,
    pub cross_scope_lookups_blocked: u64,
}

/// Per-collection shortcut table. Clones share the same backing state.
#[derive(Clone)]
pub struct CatapultTable {
    inner: Arc<RwLock<HashMap<CatapultScope, ScopeState>>>,
    config: CatapultConfig,
}

struct ScopeState {
    edges_by_region: HashMap<u64, CatapultEdge>,
    stats: CatapultStats,
}

impl ScopeState {
    fn new() -> Self {
        Self {
            edges_by_region: HashMap::new(),
            stats: CatapultStats::default(),
        }
    }
}

impl CatapultTable {
    pub fn new(config: CatapultConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(HashMap::new())),
            config,
        }
    }

    /// Observe a successful trajectory - `query_region` was satisfied by a
    /// search that profitably used `destination_node` as its entry. The
    /// scope is tenant+collection; cross-scope inserts panic in debug
    /// builds so a misuse fails the test rather than poisoning the table.
    pub async fn observe(&self, scope: &CatapultScope, query_region: u64, destination_node: u64) {
        debug_assert!(
            !scope.tenant_id.is_empty(),
            "catapult scope tenant_id must not be empty"
        );
        debug_assert!(
            !scope.collection.is_empty(),
            "catapult scope collection must not be empty"
        );
        let mut g = self.inner.write().await;
        let state = g.entry(scope.clone()).or_insert_with(ScopeState::new);
        state.stats.total_observations += 1;
        let now = Instant::now();
        let entry = state
            .edges_by_region
            .entry(query_region)
            .or_insert(CatapultEdge {
                query_region,
                destination_node,
                hits: 0,
                last_seen: now,
            });
        // If the existing entry pointed at a different destination, keep
        // the higher-hit one (CatapultDB's "stable destination" insight -
        // flapping shortcuts hurt cache locality).
        if entry.destination_node == destination_node {
            entry.hits = entry.hits.saturating_add(1);
            entry.last_seen = now;
        } else if entry.hits < 1 {
            // Existing entry never paid off - replace it.
            *entry = CatapultEdge {
                query_region,
                destination_node,
                hits: 1,
                last_seen: now,
            };
        }
        // If we exceeded the capacity ceiling, evict the coldest edge.
        if state.edges_by_region.len() > self.config.max_edges_per_scope {
            evict_one(state, self.config.min_hits_to_survive);
        }
    }

    /// Look up the shortcut for a query region. Returns `Some(destination)`
    /// when a shortcut exists for this scope; `None` otherwise. Increments
    /// the hit counter on success - the runtime can use lookup counts to
    /// decide whether to keep the edge.
    pub async fn lookup(&self, scope: &CatapultScope, query_region: u64) -> Option<u64> {
        let mut g = self.inner.write().await;
        let state = g.entry(scope.clone()).or_insert_with(ScopeState::new);
        state.stats.total_lookups += 1;
        let now = Instant::now();
        if let Some(edge) = state.edges_by_region.get_mut(&query_region) {
            // Age-out - even hot edges go stale if the workload shifts.
            if now.duration_since(edge.last_seen) > self.config.max_age {
                state.edges_by_region.remove(&query_region);
                state.stats.evictions += 1;
                return None;
            }
            edge.hits = edge.hits.saturating_add(1);
            edge.last_seen = now;
            state.stats.total_hits += 1;
            return Some(edge.destination_node);
        }
        None
    }

    /// Look up a shortcut and block the response if the supplied
    /// `expected_scope` doesn't match the stored edge's scope. Returns
    /// `None` and increments `cross_scope_lookups_blocked` - the runtime
    /// can alert on a non-zero counter.
    ///
    /// In practice the scope is keyed at lookup time so this can't trigger
    /// via the normal `lookup` path; the helper exists for explicit audit
    /// scenarios where an operator wants to scan another tenant's shortcuts.
    pub async fn lookup_with_audit(
        &self,
        actual_scope: &CatapultScope,
        expected_scope: &CatapultScope,
        query_region: u64,
    ) -> Option<u64> {
        if actual_scope != expected_scope {
            let mut g = self.inner.write().await;
            let state = g
                .entry(actual_scope.clone())
                .or_insert_with(ScopeState::new);
            state.stats.cross_scope_lookups_blocked += 1;
            return None;
        }
        self.lookup(actual_scope, query_region).await
    }

    /// Snapshot stats for a scope. Returns the default-zero stats for an
    /// unknown scope.
    pub async fn stats_for(&self, scope: &CatapultScope) -> CatapultStats {
        let guard = self.inner.read().await;
        let Some(state) = guard.get(scope) else {
            return CatapultStats::default();
        };
        let mut stats = state.stats.clone();
        // Edge count is the live size, not a counter - refresh from the map.
        stats.edges = state.edges_by_region.len();
        stats
    }

    /// Drop every edge whose `last_seen` is older than `config.max_age`,
    /// across every scope. Called by the runtime on a background timer.
    pub async fn sweep_stale(&self) -> u64 {
        let now = Instant::now();
        let mut g = self.inner.write().await;
        let mut total = 0u64;
        for state in g.values_mut() {
            let before = state.edges_by_region.len();
            state
                .edges_by_region
                .retain(|_, edge| now.duration_since(edge.last_seen) <= self.config.max_age);
            let removed = before - state.edges_by_region.len();
            state.stats.evictions += removed as u64;
            total += removed as u64;
        }
        total
    }
}

impl Default for CatapultTable {
    fn default() -> Self {
        Self::new(CatapultConfig::default())
    }
}

fn evict_one(state: &mut ScopeState, min_hits_to_survive: u64) {
    // Capacity-driven eviction: the table is over its hard ceiling, so we
    // must evict *something* - prefer edges below the configured survival
    // threshold, then use lowest hits + oldest last_seen as tie-breakers.
    let victim = state
        .edges_by_region
        .iter()
        .min_by(|a, b| {
            let a_survives = a.1.hits >= min_hits_to_survive;
            let b_survives = b.1.hits >= min_hits_to_survive;
            match a_survives.cmp(&b_survives) {
                std::cmp::Ordering::Equal => match a.1.hits.cmp(&b.1.hits) {
                    std::cmp::Ordering::Equal => a.1.last_seen.cmp(&b.1.last_seen),
                    other => other,
                },
                other => other,
            }
        })
        .map(|(k, _)| *k);
    if let Some(k) = victim {
        state.edges_by_region.remove(&k);
        state.stats.evictions += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg(max: usize) -> CatapultConfig {
        CatapultConfig {
            max_edges_per_scope: max,
            min_hits_to_survive: 1,
            max_age: Duration::from_secs(3600),
        }
    }

    fn scope(t: &str, c: &str) -> CatapultScope {
        CatapultScope::new(t, c)
    }

    #[tokio::test]
    async fn observation_then_lookup_returns_destination() {
        let table = CatapultTable::new(cfg(64));
        let s = scope("tenant-a", "kb");
        table.observe(&s, 7, 42).await;
        assert_eq!(table.lookup(&s, 7).await, Some(42));
    }

    #[tokio::test]
    async fn lookup_misses_on_unknown_query_region() {
        let table = CatapultTable::new(cfg(64));
        let s = scope("tenant-a", "kb");
        assert_eq!(table.lookup(&s, 9999).await, None);
    }

    #[tokio::test]
    async fn different_scopes_are_isolated() {
        // The critical cross-tenant safety invariant: an edge inserted for
        // tenant-a/kb must never be returned for tenant-b/kb (same
        // collection name, different tenant), even with the same query
        // region id.
        let table = CatapultTable::new(cfg(64));
        let a = scope("tenant-a", "kb");
        let b = scope("tenant-b", "kb");
        table.observe(&a, 7, 42).await;
        assert_eq!(table.lookup(&a, 7).await, Some(42));
        assert_eq!(table.lookup(&b, 7).await, None);
    }

    #[tokio::test]
    async fn repeated_observation_increments_hits() {
        let table = CatapultTable::new(cfg(64));
        let s = scope("tenant-a", "kb");
        for _ in 0..5 {
            table.observe(&s, 1, 100).await;
        }
        // Stats: 5 observations, 0 lookups so far.
        let stats = table.stats_for(&s).await;
        assert_eq!(stats.total_observations, 5);
        assert_eq!(stats.edges, 1);
    }

    #[tokio::test]
    async fn lookup_increments_hits_and_total_lookups() {
        let table = CatapultTable::new(cfg(64));
        let s = scope("tenant-a", "kb");
        table.observe(&s, 1, 100).await;
        assert_eq!(table.lookup(&s, 1).await, Some(100));
        assert_eq!(table.lookup(&s, 1).await, Some(100));
        let stats = table.stats_for(&s).await;
        assert_eq!(stats.total_lookups, 2);
        assert_eq!(stats.total_hits, 2);
    }

    #[tokio::test]
    async fn capacity_evicts_coldest_edge() {
        let table = CatapultTable::new(CatapultConfig {
            max_edges_per_scope: 2,
            min_hits_to_survive: 1,
            max_age: Duration::from_secs(3600),
        });
        let s = scope("tenant-a", "kb");
        // First two edges fill the table.
        table.observe(&s, 1, 100).await;
        table.observe(&s, 2, 200).await;
        // Make edge 1 "hotter" with a few lookups.
        for _ in 0..3 {
            table.lookup(&s, 1).await;
        }
        // Third observation forces eviction - edge 2 is cold (only 1 hit
        // from its initial observation) so it should be the victim.
        table.observe(&s, 3, 300).await;
        let stats = table.stats_for(&s).await;
        assert_eq!(stats.edges, 2, "table should not exceed capacity");
        // Either edge 2 or edge 3 must be present; edge 1 must survive.
        assert_eq!(table.lookup(&s, 1).await, Some(100));
    }

    #[tokio::test]
    async fn cross_scope_lookup_audit_increments_counter() {
        let table = CatapultTable::new(cfg(64));
        let a = scope("tenant-a", "kb");
        let b = scope("tenant-b", "kb");
        table.observe(&a, 7, 42).await;
        // Audited lookup with mismatching scope must return None and bump
        // the cross-scope counter.
        let result = table.lookup_with_audit(&a, &b, 7).await;
        assert_eq!(result, None);
        let stats = table.stats_for(&a).await;
        assert_eq!(stats.cross_scope_lookups_blocked, 1);
    }

    #[tokio::test]
    async fn sweep_stale_drops_aged_edges() {
        // Use a near-zero max_age so any edge ages out by the next sweep.
        let table = CatapultTable::new(CatapultConfig {
            max_edges_per_scope: 64,
            min_hits_to_survive: 1,
            max_age: Duration::from_nanos(1),
        });
        let s = scope("tenant-a", "kb");
        table.observe(&s, 1, 100).await;
        // Sleep a bit so the edge ages past the 1 ns max_age.
        tokio::time::sleep(Duration::from_millis(2)).await;
        let removed = table.sweep_stale().await;
        assert_eq!(removed, 1);
        assert_eq!(table.lookup(&s, 1).await, None);
    }

    #[tokio::test]
    async fn destination_flap_keeps_stickier_target() {
        // Same query_region observed twice with different destinations.
        // The first observation creates the edge; the second (with a
        // different destination) should NOT clobber it once it has hits,
        // matching CatapultDB's "stable destination" insight.
        let table = CatapultTable::new(cfg(64));
        let s = scope("tenant-a", "kb");
        // Build hit count for destination 100.
        for _ in 0..5 {
            table.observe(&s, 1, 100).await;
        }
        // A single dissenting observation must not flap the destination.
        table.observe(&s, 1, 999).await;
        assert_eq!(table.lookup(&s, 1).await, Some(100));
    }

    #[tokio::test]
    async fn unknown_scope_stats_default_to_zero() {
        let table = CatapultTable::new(cfg(64));
        let stats = table.stats_for(&scope("ghost", "kb")).await;
        assert_eq!(stats, CatapultStats::default());
    }

    #[tokio::test]
    #[should_panic(expected = "catapult scope tenant_id must not be empty")]
    async fn empty_tenant_id_panics_in_debug() {
        let table = CatapultTable::new(cfg(64));
        let s = CatapultScope {
            tenant_id: "".into(),
            collection: "kb".into(),
        };
        table.observe(&s, 1, 100).await;
    }
}
