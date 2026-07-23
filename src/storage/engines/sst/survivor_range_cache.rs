//! ADR-065 Q3 — engine-level ranged cache for coalesced-RaBitQ survivor/OID
//! byte ranges.
//!
//! ## Why this exists (co-design, first principles)
//!
//! The dominant read-path cost in coalesced ANN search is the billed
//! per-range object-store GET (`fs.read_range`) for survivor vectors (Region B
//! SQ8) and top-k OIDs (Region D) — ~14–40 ranged GETs/query at ~1 MiB each.
//! Region B/D are **immutable per segment file**, and survivors are
//! query-dependent **but repeat across hot queries** (overlapping IVF cells ⇒
//! overlapping survivor block sets ⇒ identical coalesced ranges). That is the
//! textbook "cache repeated reads of immutable data" lever, and it moves the
//! dominant cost term for a cloud DB — GET round-trips + egress — *not* CPU.
//!
//! ## Design choices (see plan: immutable-seeking-frog.md)
//!
//! - **Engine-level, not a `FileSystem` wrapper.** The engine knows the
//!   [`CacheTier`] semantics (survivors are LRU-evictable; invariants are
//!   pinned); the FS layer sees only opaque `(path, offset, length)` and cannot
//!   tier without a layering violation. This mirrors the existing
//!   [`SegmentInvariantsCache`] injection pattern.
//! - **Reuses `TenantCache`** (moka-backed, multitenant, byte-budgeted,
//!   work-conserving LRU) via its read-through `get_or_load` — no new eviction
//!   policy, no new metering.
//! - **RAM for v1.** The billed ms-latency GET dominates cost, not the cache
//!   medium, so a RAM hit wins; the SIFT1M survivor working set fits a
//!   few-hundred-MB budget. NVMe spill for 10M+ scale swaps the backing behind
//!   the same key/budget API (follow-up).
//! - **Metering is free.** The cache sits *above* the FS backend: a HIT never
//!   runs the caller's loader, so `fs.read_range` is never called and the
//!   backend's `record_range_gets`/`record_bytes_read` never fire — the GET is
//!   simply not billed, and the caller records `record_get(CacheTier, …)`
//!   inside the loader so it fires exactly once per real fetch. No parallel
//!   cache hit/miss counters; the existing io_trace seam stays the single
//!   source of truth.
//! - **No invalidation for v1.** Segment files are immutable and uniquely
//!   named (`L0_{ts}_{hash}.pax`); a replaced segment has a new path ⇒ new
//!   keys ⇒ stale entries age out via the byte-budget LRU. No coherence risk.
//! - **Default-OFF.** Injected as `Option<Arc<…>>`; `None` ⇒ the read path is
//!   byte-for-byte unchanged. Opt in via env budget
//!   `PROXIMADB_SURVIVOR_CACHE_BUDGET_MB`.

use std::future::Future;
use std::sync::Arc;

use proximadb_cache::{CacheBudget, CacheKey, CacheKind, TenantCache};
use proximadb_storage_filesystem_types::{FilesystemError, FsResult};

/// The fair-sharing / isolation dimension for the survivor cache's
/// [`TenantCache`].
///
/// Data isolation is already structural: the [`CacheKey`] key string embeds the
/// full DrPath-scoped segment path (`data/{tenant}/{ns}/…`), so no two tenants
/// ever share a key. The `tenant` field here drives only the
/// *fair-sharing* dimension (per-tenant floors/ceilings under pool pressure).
/// v1 uses a single shared tenant (one elastic pool); per-tenant fair-sharing
/// is a follow-up that threads the real `tenant_id` into the search path.
const TENANT: &str = "survivor-cache";

/// A byte-range cache for survivor (Region B SQ8) and OID (Region D) ranges of
/// immutable coalesced segments. Read-through: on a miss the caller-supplied
/// `loader` fetches the bytes; on a hit the loader never runs (so no GET is
/// issued and nothing is billed).
///
/// Backed by the multitenant, byte-budgeted, work-conserving [`TenantCache`].
pub struct SurvivorRangeCache {
    inner: Arc<TenantCache<Arc<[u8]>>>,
}

impl SurvivorRangeCache {
    /// New cache with a `budget_bytes` byte ceiling (the shared elastic pool;
    /// also the per-tenant hard ceiling — entries beyond it bypass caching, so
    /// one collection cannot monopolize the pool).
    pub fn new(budget_bytes: u64) -> Self {
        Self {
            inner: Arc::new(TenantCache::new(CacheBudget::new(
                budget_bytes,
                budget_bytes,
            ))),
        }
    }

    /// Look up the byte range `[off, off+len)` of `path`; on a miss run `loader`
    /// and cache its result iff the tenant is under its byte ceiling. Returns
    /// the cached or freshly-loaded bytes.
    ///
    /// `kind` selects the artifact class — pass [`CacheKind::QuantizedCodes`]
    /// for survivor (SQ8) ranges and [`CacheKind::Other`] for OID ranges, so
    /// per-kind stats stay separated.
    ///
    /// The loader runs ONLY on a miss, so a caller that records the GET (e.g.
    /// `record_get(CacheTier::SurvivorPayload, len)`) inside the loader records
    /// it exactly once per real fetch — a hit records nothing.
    pub async fn get_or_fetch<F, Fut>(
        &self,
        kind: CacheKind,
        path: &str,
        off: u64,
        len: u64,
        loader: F,
    ) -> FsResult<Arc<[u8]>>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = FsResult<Vec<u8>>>,
    {
        let key = CacheKey::new(TENANT, kind, format!("{path}:{off}:{len}"));
        // Ranges are ≤ a few MiB; cap the weight at u32::MAX defensively.
        let weight: u32 = len.try_into().unwrap_or(u32::MAX);
        let result = self
            .inner
            .get_or_load(key, weight, || async {
                let bytes = loader().await?;
                Ok::<Arc<[u8]>, FilesystemError>(Arc::from(bytes))
            })
            .await;
        self.sync_prometheus();
        result
    }

    /// TD-METRICS-1: mirror the `TenantCache` hit/miss/bytes atomics into the
    /// prometheus gauges after each lookup. `tenant_stats()` iterates a
    /// handful of (tenant, kind) rows — negligible next to the ranged GET the
    /// cache exists to avoid. Gauges (not counters) because the source is a
    /// running total we sample.
    fn sync_prometheus(&self) {
        let (mut hits, mut misses, mut bytes) = (0i64, 0i64, 0i64);
        for stat in self.inner.tenant_stats() {
            hits += stat.hits as i64;
            misses += stat.misses as i64;
            bytes += stat.bytes as i64;
        }
        crate::metrics::operational_metrics::SURVIVOR_CACHE_HITS.set(hits);
        crate::metrics::operational_metrics::SURVIVOR_CACHE_MISSES.set(misses);
        crate::metrics::operational_metrics::SURVIVOR_CACHE_BYTES.set(bytes);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A hit does not re-run the loader (the GET is billed at most once per
    /// unique range), and returns identical bytes.
    #[tokio::test]
    async fn hit_skips_loader_and_returns_cached_bytes() {
        let cache = SurvivorRangeCache::new(16 * 1024 * 1024);
        let fetches = Arc::new(AtomicUsize::new(0));
        let payload = vec![42u8; 4096];

        let fetches1 = fetches.clone();
        let payload1 = payload.clone();
        let first = cache
            .get_or_fetch(CacheKind::QuantizedCodes, "seg.pax", 1024, 4096, || {
                let fetches1 = fetches1.clone();
                let payload1 = payload1.clone();
                async move {
                    fetches1.fetch_add(1, Ordering::SeqCst);
                    Ok(payload1.clone())
                }
            })
            .await
            .unwrap();

        // Same (path, off, len) — must hit the cache, NOT the loader.
        let fetches2 = fetches.clone();
        let second = cache
            .get_or_fetch(CacheKind::QuantizedCodes, "seg.pax", 1024, 4096, || {
                let fetches2 = fetches2.clone();
                async move {
                    fetches2.fetch_add(1, Ordering::SeqCst);
                    Ok(vec![0u8; 4096]) // would be wrong if it ran
                }
            })
            .await
            .unwrap();

        assert_eq!(
            fetches.load(Ordering::SeqCst),
            1,
            "loader ran once (miss then hit)"
        );
        assert_eq!(&*first, &payload, "miss returned the loaded bytes");
        assert_eq!(
            &*second, &payload,
            "hit returned the cached bytes, not the loader's"
        );
    }

    /// A different range key misses independently — no false sharing across keys.
    #[tokio::test]
    async fn distinct_ranges_miss_independently() {
        let cache = SurvivorRangeCache::new(16 * 1024 * 1024);
        let fetches = Arc::new(AtomicUsize::new(0));

        for (off, len) in [(0u64, 1024u64), (4096, 2048), (0, 1024)] {
            let f = fetches.clone();
            cache
                .get_or_fetch(CacheKind::QuantizedCodes, "seg.pax", off, len, || {
                    let f = f.clone();
                    async move {
                        f.fetch_add(1, Ordering::SeqCst);
                        Ok(vec![1u8; len as usize])
                    }
                })
                .await
                .unwrap();
        }
        // (0,1024) hits on the 3rd call; the two distinct ranges miss once each.
        assert_eq!(fetches.load(Ordering::SeqCst), 2);
    }
}
