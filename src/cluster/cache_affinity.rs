/*
 * Copyright 2025 Vijaykumar Singh
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 */

//! Cache-affinity registry — Phase 7 control surface for routing
//! read queries to the node whose caches are already warm for a
//! given collection. Matches turbopuffer's
//! `/docs/architecture#cache-affinity` model: "subsequent queries
//! route to the same query node for cache locality, but any query
//! node can serve queries from any namespace."
//!
//! ## Semantics
//!
//! The registry stores a **hint**, not a hard binding:
//!
//! * `record_query(collection_id, node_id)` — called by a node when
//!   it serves a query. Updates last-seen timestamp + query count
//!   for that (collection, node) pair.
//! * `preferred_node(collection_id) -> Option<NodeId>` — returns the
//!   most-recently-active node for that collection, if the entry is
//!   still within the TTL window. Returns `None` when no recent
//!   activity exists or the entry has expired.
//!
//! Callers MUST treat the result as a preference, not a requirement:
//! a load balancer or routing service should consult `preferred_node`
//! first, fall back to its default policy when `None`, and override
//! the preference when the affinity node is unhealthy or overloaded.
//!
//! ## Single-node vs multi-node
//!
//! In single-node deployments the registry is essentially a no-op:
//! `record_query` writes the local node-id, `preferred_node` returns
//! the local node-id, the routing service picks the same node it
//! would have anyway. No harm done, no behaviour change.
//!
//! In multi-node deployments the registry becomes useful when each
//! node's local copy reflects its own recent activity — a load
//! balancer can read the registry of a candidate node and prefer
//! sending traffic to whichever node "owns" a hot collection.
//! Cross-node synchronization of the registry is NOT in scope for
//! this slice; the per-node state alone provides directional
//! locality benefit.
//!
//! ## TTL
//!
//! Default TTL is 60 seconds. Entries older than the TTL are
//! treated as cold — `preferred_node` returns `None`. This means a
//! collection that hasn't been queried in over a minute gets a
//! fresh routing decision instead of a stale affinity. TTL is
//! configurable per registry via [`CacheAffinityRegistry::with_ttl`].

use dashmap::DashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

/// One affinity entry: which node last served queries for a
/// collection, and when.
#[derive(Debug, Clone)]
pub struct AffinityEntry {
    pub node_id: String,
    /// `Instant` at which `record_query` was last called for this
    /// (collection, node). Used for TTL eviction.
    pub last_seen: Instant,
    /// Monotonic count of queries served by this node for the
    /// collection while it remained the affinity holder. Useful for
    /// operator dashboards ("hot collection X has been served by
    /// node Y for 4250 queries in the last minute").
    pub query_count: u64,
}

/// Process-wide cache-affinity registry. One instance per ProximaDB
/// process; shared via `Arc`.
#[derive(Debug)]
pub struct CacheAffinityRegistry {
    /// Per-collection affinity. The DashMap value carries the latest
    /// observed node + activity stats.
    by_collection: DashMap<String, AffinityEntry>,
    /// How long an entry is considered "fresh." After this duration
    /// elapses without a record_query, `preferred_node` returns None.
    ttl: Duration,
}

impl Default for CacheAffinityRegistry {
    fn default() -> Self {
        Self {
            by_collection: DashMap::new(),
            ttl: Duration::from_secs(60),
        }
    }
}

impl CacheAffinityRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Configure a custom TTL. Production typically uses the default
    /// 60s; tests use shorter values to exercise expiry.
    pub fn with_ttl(ttl: Duration) -> Self {
        Self {
            by_collection: DashMap::new(),
            ttl,
        }
    }

    /// Record that `node_id` served a query for `collection_id`.
    /// Updates last-seen + increments query count. If the recorded
    /// node differs from the current affinity holder, the new node
    /// takes over (last-write-wins) and the query count restarts at
    /// 1. This is the right behaviour for the multi-node case where
    /// a different node's copy of the registry will reflect its own
    /// recent activity.
    pub fn record_query(&self, collection_id: impl Into<String>, node_id: impl Into<String>) {
        let collection_id = collection_id.into();
        let node_id = node_id.into();
        self.by_collection
            .entry(collection_id)
            .and_modify(|entry| {
                if entry.node_id == node_id {
                    entry.query_count = entry.query_count.saturating_add(1);
                    entry.last_seen = Instant::now();
                } else {
                    // New node taking over — restart the counter.
                    entry.node_id = node_id.clone();
                    entry.query_count = 1;
                    entry.last_seen = Instant::now();
                }
            })
            .or_insert_with(|| AffinityEntry {
                node_id,
                last_seen: Instant::now(),
                query_count: 1,
            });
    }

    /// Return the affinity-preferred node for `collection_id` when
    /// the most recent record is still within the TTL window.
    /// Returns `None` when no entry exists or the entry has expired.
    pub fn preferred_node(&self, collection_id: &str) -> Option<String> {
        let entry = self.by_collection.get(collection_id)?;
        if entry.last_seen.elapsed() <= self.ttl {
            Some(entry.node_id.clone())
        } else {
            None
        }
    }

    /// Read the full entry (including stats) without enforcing TTL.
    /// Used by operator dashboards to display stale entries with a
    /// "stale" flag rather than hiding them.
    pub fn entry(&self, collection_id: &str) -> Option<AffinityEntry> {
        self.by_collection.get(collection_id).map(|e| e.clone())
    }

    /// True when the entry exists AND is within TTL. Equivalent to
    /// `preferred_node(...).is_some()` but doesn't allocate the
    /// returned String.
    pub fn has_fresh_affinity(&self, collection_id: &str) -> bool {
        self.by_collection
            .get(collection_id)
            .map(|e| e.last_seen.elapsed() <= self.ttl)
            .unwrap_or(false)
    }

    /// Drop the affinity for `collection_id`. Used when an operator
    /// explicitly wants the next query to be routed by the default
    /// policy (e.g., during a planned cache eviction).
    pub fn invalidate(&self, collection_id: &str) -> bool {
        self.by_collection.remove(collection_id).is_some()
    }

    /// Total entries currently held (including expired ones, since
    /// TTL is enforced lazily at read time). Operator dashboards use
    /// this to size the registry.
    pub fn len(&self) -> usize {
        self.by_collection.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_collection.is_empty()
    }

    /// Snapshot of all entries, sorted by collection_id for
    /// deterministic output. Includes expired entries so the
    /// dashboard can mark them as such.
    pub fn list(&self) -> Vec<(String, AffinityEntry)> {
        let mut out: Vec<(String, AffinityEntry)> = self
            .by_collection
            .iter()
            .map(|e| (e.key().clone(), e.value().clone()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// Evict entries older than TTL. Optional — `preferred_node`
    /// already returns None for expired entries — but useful to call
    /// periodically so the DashMap doesn't grow unboundedly for
    /// collections that are queried once and never again.
    pub fn purge_expired(&self) -> usize {
        let mut to_remove = Vec::new();
        for entry in self.by_collection.iter() {
            if entry.value().last_seen.elapsed() > self.ttl {
                to_remove.push(entry.key().clone());
            }
        }
        let n = to_remove.len();
        for k in to_remove {
            self.by_collection.remove(&k);
        }
        n
    }
}

/// Builder for `Arc<CacheAffinityRegistry>` so the routing service
/// constructor can share the same instance across protocol handlers.
pub fn new_shared() -> Arc<CacheAffinityRegistry> {
    Arc::new(CacheAffinityRegistry::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn new_registry_is_empty() {
        let reg = CacheAffinityRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.preferred_node("any").is_none());
        assert!(!reg.has_fresh_affinity("any"));
    }

    #[test]
    fn record_query_creates_entry_with_count_one() {
        let reg = CacheAffinityRegistry::new();
        reg.record_query("coll-a", "node-1");
        let entry = reg.entry("coll-a").unwrap();
        assert_eq!(entry.node_id, "node-1");
        assert_eq!(entry.query_count, 1);
        assert!(reg.has_fresh_affinity("coll-a"));
        assert_eq!(reg.preferred_node("coll-a").as_deref(), Some("node-1"));
    }

    #[test]
    fn repeated_record_query_on_same_node_increments_count() {
        let reg = CacheAffinityRegistry::new();
        for _ in 0..5 {
            reg.record_query("coll", "node-1");
        }
        let entry = reg.entry("coll").unwrap();
        assert_eq!(entry.node_id, "node-1");
        assert_eq!(entry.query_count, 5);
    }

    #[test]
    fn record_query_on_different_node_takes_over_with_count_one() {
        // Multi-node case: node-2's local record_query reflects
        // node-2's activity. Last writer wins.
        let reg = CacheAffinityRegistry::new();
        reg.record_query("coll", "node-1");
        reg.record_query("coll", "node-1");
        reg.record_query("coll", "node-2");

        let entry = reg.entry("coll").unwrap();
        assert_eq!(entry.node_id, "node-2");
        assert_eq!(entry.query_count, 1, "new node takeover restarts count");
    }

    #[test]
    fn expired_entry_returns_no_preferred_node() {
        // Use a very short TTL to keep the test fast.
        let reg = CacheAffinityRegistry::with_ttl(Duration::from_millis(10));
        reg.record_query("coll", "node-1");
        thread::sleep(Duration::from_millis(30));
        assert!(
            reg.preferred_node("coll").is_none(),
            "expired entry must not be returned as preferred"
        );
        assert!(!reg.has_fresh_affinity("coll"));
        // Entry method ignores TTL — operator dashboards still see it.
        assert!(reg.entry("coll").is_some());
    }

    #[test]
    fn invalidate_drops_entry_for_planned_eviction() {
        let reg = CacheAffinityRegistry::new();
        reg.record_query("coll", "node-1");
        assert!(reg.invalidate("coll"));
        assert!(reg.preferred_node("coll").is_none());
        assert!(reg.entry("coll").is_none());

        // Idempotent — invalidating an unknown collection is fine.
        assert!(!reg.invalidate("never-seen"));
    }

    #[test]
    fn purge_expired_drops_only_old_entries() {
        let reg = CacheAffinityRegistry::with_ttl(Duration::from_millis(15));
        reg.record_query("old-coll", "node-1");
        thread::sleep(Duration::from_millis(25));
        reg.record_query("new-coll", "node-2");

        let purged = reg.purge_expired();
        assert_eq!(purged, 1, "exactly one stale entry should be purged");
        assert!(reg.entry("old-coll").is_none());
        assert!(reg.entry("new-coll").is_some());
    }

    #[test]
    fn list_returns_deterministic_order_by_collection_id() {
        let reg = CacheAffinityRegistry::new();
        reg.record_query("coll-c", "node-1");
        reg.record_query("coll-a", "node-1");
        reg.record_query("coll-b", "node-2");

        let listed = reg.list();
        let ids: Vec<&str> = listed.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, vec!["coll-a", "coll-b", "coll-c"]);
    }

    #[test]
    fn single_node_use_is_a_noop_in_practice() {
        // Single-node deployment: only one node-id ever recorded.
        // The registry returns it; the load balancer's "preferred
        // node" is also "the only node available," so behaviour is
        // identical to no-affinity routing. This test pins that
        // contract so future "smarter" semantics don't break the
        // single-node default.
        let reg = CacheAffinityRegistry::new();
        let self_id = "self";
        for _ in 0..100 {
            reg.record_query("coll", self_id);
        }
        assert_eq!(reg.preferred_node("coll").as_deref(), Some(self_id));
        // 100 queries → count tracks the activity.
        assert_eq!(reg.entry("coll").unwrap().query_count, 100);
    }
}
