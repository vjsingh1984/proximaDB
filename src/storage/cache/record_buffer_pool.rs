// Record-level buffer pool — LLD §4, anchored on VeloANN arXiv 2602.22805.
//
// On the SIFT1M workload, DiskANN leaves 47% of vertices unaccessed but only
// 0.1% of *pages* untouched. That mismatch means a page-level buffer cache
// holds the wrong granularity: the moment any record in a page is hot, the
// whole page stays resident, including ~half its vertices that will never
// be read. The cure is to cache at record granularity with a lightweight
// admission/eviction policy that runs in microseconds.
//
// This module ships the data-structure primitive — a generic clock /
// second-chance buffer pool — that the AXIS runtime (Phase 3 follow-up) will
// install in place of the existing page-level cache for graph nodes. The
// pool exposes the metrics the LLD §10 trace needs (`record_hits`,
// `page_hits`) and a clear pin/unpin API so the runtime can hold a record
// across a brief search step without it being evicted underneath.

use std::collections::HashMap;
use std::hash::Hash;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use tokio::sync::Mutex;

/// Per-record slot kept inside the pool.
struct Slot<K, V> {
    key: K,
    value: Arc<V>,
    /// Clock reference bit — set on every hit, cleared on the sweep.
    referenced: bool,
    /// Pin count — non-zero pins are skipped by the eviction sweep.
    pin_count: u32,
}

/// Atomic counters surfaced to the SearchPlanTrace.
#[derive(Debug, Default)]
pub struct BufferPoolStats {
    /// Hits at record granularity.
    pub record_hits: AtomicU64,
    /// Hits served by the page cache one level below us. Phase 3 wires this
    /// from the page-cache adapter; today the AXIS runtime stays at the
    /// record level so this counter remains 0 until the integration ships.
    pub page_hits: AtomicU64,
    /// Misses (record not in pool).
    pub misses: AtomicU64,
    /// Records evicted by the clock sweep.
    pub evictions: AtomicU64,
}

impl BufferPoolStats {
    pub fn snapshot(&self) -> BufferPoolSnapshot {
        BufferPoolSnapshot {
            record_hits: self.record_hits.load(Ordering::Relaxed),
            page_hits: self.page_hits.load(Ordering::Relaxed),
            misses: self.misses.load(Ordering::Relaxed),
            evictions: self.evictions.load(Ordering::Relaxed),
        }
    }
}

/// Static snapshot of the buffer-pool stats for trace emission.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BufferPoolSnapshot {
    pub record_hits: u64,
    pub page_hits: u64,
    pub misses: u64,
    pub evictions: u64,
}

impl BufferPoolSnapshot {
    /// Ratio of record-level hits vs total hits. The LLD acceptance gate
    /// asks this to clear 0.5 once Phase 3 lands on hot collections.
    pub fn record_hit_ratio(&self) -> f64 {
        let total_hits = self.record_hits + self.page_hits;
        if total_hits == 0 {
            return 0.0;
        }
        self.record_hits as f64 / total_hits as f64
    }
}

/// Pinned reference returned by `get_or_pin`. The pin is released when the
/// guard is dropped — callers can hold it across a brief search step
/// without racing with the eviction sweep.
pub struct PinnedRecord<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    pool: BufferPool<K, V>,
    key: K,
    value: Arc<V>,
}

impl<K, V> PinnedRecord<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    pub fn value(&self) -> &Arc<V> {
        &self.value
    }

    pub fn key(&self) -> &K {
        &self.key
    }
}

impl<K, V> Drop for PinnedRecord<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    fn drop(&mut self) {
        // Best-effort unpin. We can't await in Drop so we offload to a
        // detached task; the lock is contended only by the eviction sweep
        // and the unpin completes in microseconds.
        let pool = self.pool.clone();
        let key = self.key.clone();
        tokio::spawn(async move {
            pool.unpin(&key).await;
        });
    }
}

/// Clock / second-chance buffer pool, generic over key and value types.
///
/// The pool is cheap to clone — internal state is wrapped in
/// `Arc<Mutex<…>>`. The mutex is held for O(1) work on get / put and
/// O(capacity) only during eviction sweeps, so contention stays bounded.
pub struct BufferPool<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    inner: Arc<Mutex<Inner<K, V>>>,
    stats: Arc<BufferPoolStats>,
    capacity: usize,
}

impl<K, V> Clone for BufferPool<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    fn clone(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            stats: self.stats.clone(),
            capacity: self.capacity,
        }
    }
}

struct Inner<K, V> {
    /// Insertion order — also the clock hand's traversal order.
    slots: Vec<Slot<K, V>>,
    /// Reverse index so `get` is O(1).
    index: HashMap<K, usize>,
    /// Clock hand position.
    hand: usize,
}

impl<K, V> BufferPool<K, V>
where
    K: Hash + Eq + Clone + Send + Sync + 'static,
    V: Send + Sync + 'static,
{
    /// Build a pool with the given capacity. Capacity is a hard ceiling
    /// across all keys; reaching it triggers a clock sweep on the next put.
    pub fn new(capacity: usize) -> Self {
        assert!(capacity > 0, "buffer pool capacity must be > 0");
        Self {
            inner: Arc::new(Mutex::new(Inner {
                slots: Vec::with_capacity(capacity),
                index: HashMap::with_capacity(capacity),
                hand: 0,
            })),
            stats: Arc::new(BufferPoolStats::default()),
            capacity,
        }
    }

    /// Stats handle for observability.
    pub fn stats(&self) -> Arc<BufferPoolStats> {
        self.stats.clone()
    }

    /// Hard capacity ceiling. Stable for the lifetime of the pool.
    pub fn capacity(&self) -> usize {
        self.capacity
    }

    /// Look up a record. Sets the reference bit on hit. Returns `None`
    /// without changing the pool on miss.
    pub async fn get(&self, key: &K) -> Option<Arc<V>> {
        let mut g = self.inner.lock().await;
        if let Some(&idx) = g.index.get(key) {
            g.slots[idx].referenced = true;
            let v = g.slots[idx].value.clone();
            drop(g);
            self.stats.record_hits.fetch_add(1, Ordering::Relaxed);
            Some(v)
        } else {
            drop(g);
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Insert a record. If the pool is at capacity, the clock sweep evicts
    /// one unpinned non-referenced slot to make room. If every slot is
    /// pinned the put is dropped — callers must drop their pins promptly.
    pub async fn put(&self, key: K, value: Arc<V>) {
        let mut g = self.inner.lock().await;
        if let Some(&idx) = g.index.get(&key) {
            // Already present — refresh the value and set reference bit.
            g.slots[idx].value = value;
            g.slots[idx].referenced = true;
            return;
        }
        if g.slots.len() < self.capacity {
            let idx = g.slots.len();
            g.slots.push(Slot {
                key: key.clone(),
                value,
                referenced: true,
                pin_count: 0,
            });
            g.index.insert(key, idx);
            return;
        }
        // Capacity reached — clock sweep for an unpinned, non-referenced slot.
        let n = g.slots.len();
        let mut swept = 0usize;
        loop {
            let i = g.hand;
            g.hand = (g.hand + 1) % n;
            // Skip pinned slots — they participate in the sweep without
            // being eligible for eviction. We still clear their reference
            // bit so they don't accumulate priority.
            if g.slots[i].pin_count > 0 {
                g.slots[i].referenced = false;
                swept += 1;
                if swept >= 2 * n {
                    // Every slot is pinned — we cannot evict. Drop the put
                    // rather than blocking; the caller will retry on the
                    // next request.
                    return;
                }
                continue;
            }
            if g.slots[i].referenced {
                g.slots[i].referenced = false;
                swept += 1;
                if swept >= 2 * n {
                    // We've swept twice — pick the next unpinned slot.
                    // Search forward; we know at least one unpinned slot
                    // exists because we checked above.
                    let mut j = g.hand;
                    for _ in 0..n {
                        if g.slots[j].pin_count == 0 {
                            evict_at(&mut g, j);
                            g.hand = (j + 1) % n;
                            self.stats.evictions.fetch_add(1, Ordering::Relaxed);
                            let new_slot = Slot {
                                key: key.clone(),
                                value,
                                referenced: true,
                                pin_count: 0,
                            };
                            g.slots[j] = new_slot;
                            g.index.insert(key, j);
                            return;
                        }
                        j = (j + 1) % n;
                    }
                    return;
                }
                continue;
            }
            // Slot is unpinned + not referenced — evict it.
            evict_at(&mut g, i);
            self.stats.evictions.fetch_add(1, Ordering::Relaxed);
            g.slots[i] = Slot {
                key: key.clone(),
                value,
                referenced: true,
                pin_count: 0,
            };
            g.index.insert(key, i);
            return;
        }
    }

    /// Pin a record so it survives the next eviction sweep. Returns a guard
    /// that auto-releases on drop. If the record isn't present, returns
    /// `None` — callers should `put` first or use `get_or_pin`.
    pub async fn pin(&self, key: &K) -> Option<PinnedRecord<K, V>> {
        let mut g = self.inner.lock().await;
        if let Some(&idx) = g.index.get(key) {
            g.slots[idx].pin_count = g.slots[idx].pin_count.saturating_add(1);
            g.slots[idx].referenced = true;
            let value = g.slots[idx].value.clone();
            drop(g);
            self.stats.record_hits.fetch_add(1, Ordering::Relaxed);
            Some(PinnedRecord {
                pool: self.clone(),
                key: key.clone(),
                value,
            })
        } else {
            drop(g);
            self.stats.misses.fetch_add(1, Ordering::Relaxed);
            None
        }
    }

    /// Internal pin release — called by the PinnedRecord drop.
    pub(crate) async fn unpin(&self, key: &K) {
        let mut g = self.inner.lock().await;
        if let Some(&idx) = g.index.get(key) {
            g.slots[idx].pin_count = g.slots[idx].pin_count.saturating_sub(1);
        }
    }

    /// Number of currently-cached records. Useful for tests + dashboards.
    pub async fn len(&self) -> usize {
        self.inner.lock().await.slots.len()
    }

    /// True when no records are currently cached.
    pub async fn is_empty(&self) -> bool {
        self.inner.lock().await.slots.is_empty()
    }

    /// Record a page-level hit. Phase 3+ integration will call this from
    /// the page-cache adapter so the LLD trace can compute the record-vs-page
    /// hit-ratio.
    pub fn record_page_hit(&self) {
        self.stats.page_hits.fetch_add(1, Ordering::Relaxed);
    }
}

fn evict_at<K, V>(inner: &mut Inner<K, V>, idx: usize)
where
    K: Hash + Eq + Clone,
{
    let key = inner.slots[idx].key.clone();
    inner.index.remove(&key);
}

#[cfg(test)]
mod tests {
    use super::*;

    type Pool = BufferPool<u64, Vec<u8>>;

    #[tokio::test]
    async fn miss_returns_none_and_increments_miss_counter() {
        let pool: Pool = BufferPool::new(4);
        assert!(pool.get(&42).await.is_none());
        assert_eq!(pool.stats().snapshot().misses, 1);
        assert_eq!(pool.stats().snapshot().record_hits, 0);
    }

    #[tokio::test]
    async fn put_then_get_is_a_hit() {
        let pool: Pool = BufferPool::new(4);
        pool.put(1, Arc::new(vec![1u8])).await;
        let v = pool.get(&1).await.expect("hit");
        assert_eq!(v.as_ref(), &vec![1u8]);
        assert_eq!(pool.stats().snapshot().record_hits, 1);
    }

    #[tokio::test]
    async fn capacity_is_bounded_and_eviction_happens() {
        // Insert n+1 records into a pool of size n; assert exactly n remain
        // and one eviction was recorded. Under standard second-chance with
        // all reference bits initially set, the victim is deterministic but
        // depends on the hand-start convention — we test the invariants the
        // pool guarantees (capacity bound + eviction counter), not the
        // identity of the specific victim.
        let pool: Pool = BufferPool::new(3);
        pool.put(1, Arc::new(vec![1u8])).await;
        pool.put(2, Arc::new(vec![2u8])).await;
        pool.put(3, Arc::new(vec![3u8])).await;
        assert_eq!(pool.len().await, 3);
        pool.put(4, Arc::new(vec![4u8])).await;
        assert_eq!(pool.len().await, 3, "capacity must not be exceeded");
        assert_eq!(
            pool.stats().snapshot().evictions,
            1,
            "one eviction expected"
        );
        // The freshest insertion must always be present.
        assert!(pool.get(&4).await.is_some());
    }

    #[tokio::test]
    async fn touched_items_survive_subsequent_eviction_rounds() {
        // After a sweep clears all reference bits, the next touch protects
        // those keys against the *next* eviction round. Cap=3, insert 1,2,3,
        // force a sweep with put(4), then touch 4 and put(5) — 4 must
        // survive because its reference bit was set by the touch.
        let pool: Pool = BufferPool::new(3);
        pool.put(1, Arc::new(vec![1u8])).await;
        pool.put(2, Arc::new(vec![2u8])).await;
        pool.put(3, Arc::new(vec![3u8])).await;
        pool.put(4, Arc::new(vec![4u8])).await; // forces one eviction
        let _ = pool.get(&4).await; // protect 4 for the next round
        pool.put(5, Arc::new(vec![5u8])).await; // forces another eviction
        assert_eq!(pool.len().await, 3);
        assert!(
            pool.get(&4).await.is_some(),
            "recently-touched key 4 should survive"
        );
        assert!(
            pool.get(&5).await.is_some(),
            "freshly-inserted key 5 should be present"
        );
    }

    #[tokio::test]
    async fn pinned_records_survive_eviction() {
        let pool: Pool = BufferPool::new(3);
        pool.put(1, Arc::new(vec![1u8])).await;
        pool.put(2, Arc::new(vec![2u8])).await;
        pool.put(3, Arc::new(vec![3u8])).await;
        let pin = pool.pin(&2).await.expect("pinned");
        assert_eq!(pin.value().as_ref(), &vec![2u8]);
        // Fill until eviction pressure forces a sweep. 2 must not be picked.
        pool.put(4, Arc::new(vec![4u8])).await;
        pool.put(5, Arc::new(vec![5u8])).await;
        assert!(pool.get(&2).await.is_some(), "pinned record was evicted");
        drop(pin);
        // After unpin completes (yield once for the spawned task) the slot
        // can be picked by the sweep on the next pressure.
        tokio::task::yield_now().await;
    }

    #[tokio::test]
    async fn put_replaces_existing_value() {
        let pool: Pool = BufferPool::new(2);
        pool.put(7, Arc::new(vec![1u8])).await;
        pool.put(7, Arc::new(vec![2u8])).await;
        let v = pool.get(&7).await.expect("present");
        assert_eq!(v.as_ref(), &vec![2u8]);
        assert_eq!(pool.len().await, 1);
    }

    #[tokio::test]
    async fn snapshot_record_hit_ratio_is_zero_when_no_hits() {
        let pool: Pool = BufferPool::new(2);
        let _ = pool.get(&1).await; // miss
        let snap = pool.stats().snapshot();
        assert_eq!(snap.record_hit_ratio(), 0.0);
    }

    #[tokio::test]
    async fn record_hit_ratio_reflects_mix() {
        let pool: Pool = BufferPool::new(2);
        pool.put(1, Arc::new(vec![1])).await;
        let _ = pool.get(&1).await; // record hit
        pool.record_page_hit(); // simulated page-cache hit
        let snap = pool.stats().snapshot();
        // 1 record_hit, 1 page_hit -> ratio 0.5
        assert!((snap.record_hit_ratio() - 0.5).abs() < 1e-9);
    }

    #[tokio::test]
    async fn over_full_pinned_drops_silently_rather_than_panicking() {
        // Pool of 1; pinning that one entry then putting a new one cannot
        // succeed — the pool must drop the put rather than panic or block.
        let pool: Pool = BufferPool::new(1);
        pool.put(1, Arc::new(vec![1u8])).await;
        let _pin = pool.pin(&1).await.expect("pinned");
        pool.put(2, Arc::new(vec![2u8])).await; // dropped silently
        assert!(pool.get(&1).await.is_some());
        assert!(pool.get(&2).await.is_none());
    }

    #[tokio::test]
    async fn clone_shares_underlying_pool() {
        let pool: Pool = BufferPool::new(3);
        let pool2 = pool.clone();
        pool.put(1, Arc::new(vec![1])).await;
        assert!(pool2.get(&1).await.is_some());
        // Stats are shared too.
        assert_eq!(pool2.stats().snapshot().record_hits, 1);
    }

    #[tokio::test]
    async fn eviction_counter_increments_only_on_actual_eviction() {
        let pool: Pool = BufferPool::new(2);
        pool.put(1, Arc::new(vec![1])).await;
        pool.put(2, Arc::new(vec![2])).await;
        assert_eq!(pool.stats().snapshot().evictions, 0);
        pool.put(3, Arc::new(vec![3])).await; // triggers one eviction
        assert_eq!(pool.stats().snapshot().evictions, 1);
    }
}
