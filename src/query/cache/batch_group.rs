// Batch-group cache tier - LLD 6.3, anchored on CALL arXiv 2509.18670.
//
// Stream-based RAG workloads process queries in batches (e.g. a turn-based
// agent fans out 32 queries for one round of reasoning). Within a batch,
// embedding models often map distinct queries to similar regions of vector
// space - so the queries access overlapping cluster files on disk. CALL's
// insight: group batched queries by their cluster-access patterns and
// emit prefetch hints at group transitions. Reported: p99 tail latency
// reduced by 33% on stream-based RAG.
//
// This module ships the per-batch cache tier the LLD 6 hardening calls
// out. Keys are `(batch_id, group_id)`; values are the shared set of
// cluster file ids that the group will access plus an optional prefetch
// hint pointing at the next group's clusters. Eviction is end-of-batch:
// once a batch's window closes, every entry for that batch is dropped.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::RwLock;

/// One group inside a batch - queries that share a cluster-access pattern.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct GroupKey {
    pub batch_id: String,
    pub group_id: u32,
}

impl GroupKey {
    pub fn new(batch_id: impl Into<String>, group_id: u32) -> Self {
        Self {
            batch_id: batch_id.into(),
            group_id,
        }
    }
}

/// What the cache stores per group: the cluster id set and a prefetch
/// hint pointing at the next group's clusters (`None` for the last group
/// in the batch).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GroupEntry {
    /// Cluster file ids the group reads.
    pub cluster_ids: Vec<u64>,
    /// Hint: clusters the *next* group will likely touch - emitted to the
    /// runtime so it can prefetch ahead of the group transition.
    pub next_group_prefetch: Option<Vec<u64>>,
    /// When the entry was admitted.
    pub admitted_at: Instant,
}

/// Tunable knobs.
#[derive(Debug, Clone, Copy)]
pub struct BatchGroupConfig {
    /// Maximum batches kept live before the oldest is force-closed. Keeps
    /// memory bounded when a runtime forgets to close a batch.
    pub max_open_batches: usize,
    /// How long an unmodified batch stays alive before auto-close.
    pub batch_idle_timeout: Duration,
}

impl Default for BatchGroupConfig {
    fn default() -> Self {
        Self {
            max_open_batches: 1024,
            batch_idle_timeout: Duration::from_secs(60),
        }
    }
}

/// Per-cache counters surfaced for observability.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BatchGroupStats {
    pub open_batches: usize,
    pub total_admissions: u64,
    pub total_lookups: u64,
    pub total_hits: u64,
    pub total_batches_closed: u64,
    pub total_prefetch_hints_emitted: u64,
    pub idle_evictions: u64,
}

/// Per-batch state. Owned inside the table; never exposed externally.
struct BatchState {
    groups: HashMap<u32, GroupEntry>,
    last_touched: Instant,
}

impl BatchState {
    fn new() -> Self {
        Self {
            groups: HashMap::new(),
            last_touched: Instant::now(),
        }
    }
}

/// Cache for streaming-RAG batch groups. Cheap to clone (wraps an
/// `Arc<RwLock<Inner>>`). Single-process; multi-node sharing isn't needed
/// because a batch lives on one gateway node by construction.
#[derive(Clone)]
pub struct BatchGroupCache {
    inner: Arc<RwLock<Inner>>,
    config: BatchGroupConfig,
}

struct Inner {
    batches: HashMap<String, BatchState>,
    stats: BatchGroupStats,
}

impl BatchGroupCache {
    pub fn new(config: BatchGroupConfig) -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                batches: HashMap::new(),
                stats: BatchGroupStats::default(),
            })),
            config,
        }
    }

    /// Admit a group entry. If the parent batch was previously closed, the
    /// admission re-opens it implicitly. Re-admitting the same `(batch, group)`
    /// overwrites the prior entry.
    pub async fn admit(&self, key: &GroupKey, entry: GroupEntry) {
        let mut g = self.inner.write().await;
        // If we're at the max-open ceiling and the batch is new, evict the
        // oldest one (LRU by `last_touched`) to make room.
        if !g.batches.contains_key(&key.batch_id) && g.batches.len() >= self.config.max_open_batches
        {
            let victim_id = g
                .batches
                .iter()
                .min_by_key(|(_, s)| s.last_touched)
                .map(|(id, _)| id.clone());
            if let Some(id) = victim_id {
                g.batches.remove(&id);
                g.stats.total_batches_closed += 1;
                g.stats.idle_evictions += 1;
            }
        }
        let state = g
            .batches
            .entry(key.batch_id.clone())
            .or_insert_with(BatchState::new);
        state.last_touched = Instant::now();
        state.groups.insert(key.group_id, entry);
        g.stats.total_admissions += 1;
        g.stats.open_batches = g.batches.len();
    }

    /// Look up a group's cluster set + prefetch hint. Increments hit
    /// counters on success and refreshes the batch's `last_touched`.
    pub async fn lookup(&self, key: &GroupKey) -> Option<GroupEntry> {
        let mut g = self.inner.write().await;
        g.stats.total_lookups += 1;
        let state = g.batches.get_mut(&key.batch_id)?;
        let entry = state.groups.get(&key.group_id)?.clone();
        state.last_touched = Instant::now();
        g.stats.total_hits += 1;
        if entry.next_group_prefetch.is_some() {
            g.stats.total_prefetch_hints_emitted += 1;
        }
        Some(entry)
    }

    /// Explicitly close a batch - drops every group entry under it. The
    /// runtime calls this when the batch's window closes (caller knows
    /// the agentic loop is done emitting queries for this batch).
    pub async fn close_batch(&self, batch_id: &str) {
        let mut g = self.inner.write().await;
        if g.batches.remove(batch_id).is_some() {
            g.stats.total_batches_closed += 1;
            g.stats.open_batches = g.batches.len();
        }
    }

    /// Drop every batch whose `last_touched` is older than the configured
    /// idle timeout. The runtime calls this on a background timer.
    pub async fn sweep_idle(&self) -> u64 {
        let now = Instant::now();
        let mut g = self.inner.write().await;
        let stale: Vec<String> = g
            .batches
            .iter()
            .filter(|(_, s)| now.duration_since(s.last_touched) > self.config.batch_idle_timeout)
            .map(|(id, _)| id.clone())
            .collect();
        let evicted = stale.len() as u64;
        for id in stale {
            g.batches.remove(&id);
        }
        g.stats.idle_evictions += evicted;
        g.stats.total_batches_closed += evicted;
        g.stats.open_batches = g.batches.len();
        evicted
    }

    /// Snapshot the counters.
    pub async fn stats(&self) -> BatchGroupStats {
        let g = self.inner.read().await;
        let mut stats = g.stats.clone();
        stats.open_batches = g.batches.len();
        stats
    }

    /// Number of live groups across all open batches. Useful for tests
    /// and capacity dashboards.
    pub async fn live_groups(&self) -> usize {
        self.inner
            .read()
            .await
            .batches
            .values()
            .map(|s| s.groups.len())
            .sum()
    }
}

impl Default for BatchGroupCache {
    fn default() -> Self {
        Self::new(BatchGroupConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(clusters: Vec<u64>, prefetch: Option<Vec<u64>>) -> GroupEntry {
        GroupEntry {
            cluster_ids: clusters,
            next_group_prefetch: prefetch,
            admitted_at: Instant::now(),
        }
    }

    fn cfg(max: usize, idle_ms: u64) -> BatchGroupConfig {
        BatchGroupConfig {
            max_open_batches: max,
            batch_idle_timeout: Duration::from_millis(idle_ms),
        }
    }

    #[tokio::test]
    async fn admit_then_lookup_returns_entry() {
        let cache = BatchGroupCache::default();
        let k = GroupKey::new("batch-1", 0);
        cache.admit(&k, entry(vec![1, 2, 3], None)).await;
        let got = cache.lookup(&k).await.expect("hit");
        assert_eq!(got.cluster_ids, vec![1, 2, 3]);
        assert_eq!(got.next_group_prefetch, None);
    }

    #[tokio::test]
    async fn lookup_misses_on_unknown_batch() {
        let cache = BatchGroupCache::default();
        assert!(cache.lookup(&GroupKey::new("ghost", 0)).await.is_none());
        let stats = cache.stats().await;
        assert_eq!(stats.total_lookups, 1);
        assert_eq!(stats.total_hits, 0);
    }

    #[tokio::test]
    async fn lookup_misses_on_unknown_group_within_known_batch() {
        let cache = BatchGroupCache::default();
        cache
            .admit(&GroupKey::new("b1", 0), entry(vec![1], None))
            .await;
        assert!(cache.lookup(&GroupKey::new("b1", 99)).await.is_none());
    }

    #[tokio::test]
    async fn re_admit_overwrites_existing_entry() {
        let cache = BatchGroupCache::default();
        let k = GroupKey::new("b1", 0);
        cache.admit(&k, entry(vec![1], None)).await;
        cache.admit(&k, entry(vec![2, 3], Some(vec![4]))).await;
        let got = cache.lookup(&k).await.unwrap();
        assert_eq!(got.cluster_ids, vec![2, 3]);
        assert_eq!(got.next_group_prefetch, Some(vec![4]));
    }

    #[tokio::test]
    async fn close_batch_drops_all_groups_in_batch() {
        let cache = BatchGroupCache::default();
        cache
            .admit(&GroupKey::new("b1", 0), entry(vec![1], None))
            .await;
        cache
            .admit(&GroupKey::new("b1", 1), entry(vec![2], None))
            .await;
        cache
            .admit(&GroupKey::new("b2", 0), entry(vec![3], None))
            .await;
        cache.close_batch("b1").await;
        assert!(cache.lookup(&GroupKey::new("b1", 0)).await.is_none());
        assert!(cache.lookup(&GroupKey::new("b1", 1)).await.is_none());
        // b2 still alive.
        assert!(cache.lookup(&GroupKey::new("b2", 0)).await.is_some());
        let stats = cache.stats().await;
        assert_eq!(stats.total_batches_closed, 1);
    }

    #[tokio::test]
    async fn close_batch_on_unknown_id_is_a_noop() {
        let cache = BatchGroupCache::default();
        cache.close_batch("never-was").await;
        let stats = cache.stats().await;
        assert_eq!(stats.total_batches_closed, 0);
    }

    #[tokio::test]
    async fn max_open_batches_evicts_oldest_when_exceeded() {
        let cache = BatchGroupCache::new(cfg(2, 60_000));
        cache
            .admit(&GroupKey::new("b1", 0), entry(vec![1], None))
            .await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        cache
            .admit(&GroupKey::new("b2", 0), entry(vec![2], None))
            .await;
        tokio::time::sleep(Duration::from_millis(2)).await;
        // Third batch forces eviction of the oldest (b1).
        cache
            .admit(&GroupKey::new("b3", 0), entry(vec![3], None))
            .await;
        assert!(cache.lookup(&GroupKey::new("b1", 0)).await.is_none());
        assert!(cache.lookup(&GroupKey::new("b2", 0)).await.is_some());
        assert!(cache.lookup(&GroupKey::new("b3", 0)).await.is_some());
        let stats = cache.stats().await;
        assert_eq!(stats.idle_evictions, 1);
    }

    #[tokio::test]
    async fn sweep_idle_drops_stale_batches() {
        let cache = BatchGroupCache::new(cfg(64, 1)); // 1 ms idle
        cache
            .admit(&GroupKey::new("b1", 0), entry(vec![1], None))
            .await;
        tokio::time::sleep(Duration::from_millis(3)).await;
        let removed = cache.sweep_idle().await;
        assert_eq!(removed, 1);
        assert!(cache.lookup(&GroupKey::new("b1", 0)).await.is_none());
    }

    #[tokio::test]
    async fn sweep_idle_preserves_fresh_batches() {
        let cache = BatchGroupCache::new(cfg(64, 60_000));
        cache
            .admit(&GroupKey::new("b1", 0), entry(vec![1], None))
            .await;
        let removed = cache.sweep_idle().await;
        assert_eq!(removed, 0);
        assert!(cache.lookup(&GroupKey::new("b1", 0)).await.is_some());
    }

    #[tokio::test]
    async fn prefetch_hint_emission_is_counted() {
        let cache = BatchGroupCache::default();
        cache
            .admit(&GroupKey::new("b1", 0), entry(vec![1], Some(vec![2, 3])))
            .await;
        cache.lookup(&GroupKey::new("b1", 0)).await;
        let stats = cache.stats().await;
        assert_eq!(stats.total_prefetch_hints_emitted, 1);
    }

    #[tokio::test]
    async fn lookup_without_prefetch_does_not_count_hint() {
        let cache = BatchGroupCache::default();
        cache
            .admit(&GroupKey::new("b1", 0), entry(vec![1], None))
            .await;
        cache.lookup(&GroupKey::new("b1", 0)).await;
        let stats = cache.stats().await;
        assert_eq!(stats.total_prefetch_hints_emitted, 0);
    }

    #[tokio::test]
    async fn live_groups_counts_across_batches() {
        let cache = BatchGroupCache::default();
        cache
            .admit(&GroupKey::new("b1", 0), entry(vec![1], None))
            .await;
        cache
            .admit(&GroupKey::new("b1", 1), entry(vec![2], None))
            .await;
        cache
            .admit(&GroupKey::new("b2", 0), entry(vec![3], None))
            .await;
        assert_eq!(cache.live_groups().await, 3);
        cache.close_batch("b1").await;
        assert_eq!(cache.live_groups().await, 1);
    }
}
