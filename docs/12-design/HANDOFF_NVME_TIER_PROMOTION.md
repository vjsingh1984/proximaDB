# Handoff: Local-Disk Tier Promotion — DRAM→Local-Disk Spill for Both Cache Tiers

## Context

The SIFT1M end-to-end benchmark (2026-07-29) measured **48.5 GETs/query cold** at 1M scale (2 segments, `level_multiplier=1.0`). The dominant cost driver is SQ8 survivor range misses (38.5 GETs/query at 32% DRAM cache hit) + control-plane misses (4 GETs/query at 0% invariants cache hit — a separate bug). Each cold miss = 1 Azure Blob GET ($0.005/10K).

Azure instance-store NVMe can provide the local path without a separate
managed-disk line item on suitable VM SKUs. It is bundled capacity, not free
hardware; HDD, SATA SSD, managed disk, and instance NVMe all satisfy the
correctness contract, with materially different latency and VM economics.

An L1→L2 spillover pattern already exists in `src/storage/cache/base.rs`.
The similarly named `NvmeBackend` was an initial reuse candidate, but the
implementation audit below found that it is memory-backed and unsuitable for
restart persistence.

## The Goal

Wire a persistent local-disk tier into BOTH warm-tier caches:

```
Object Store (Azure Blob, $0.005/10K GETs)
    ↑ cold miss (first-ever access to a segment)
    |
Local-disk L2 (instance-store NVMe is the recommended profile)
    ↑ warm miss (evicted from DRAM, still on local disk)
    |
DRAM L1 (moka — admission-controlled by CacheTier priority)
    = hot hit (the working set)
```

Both `SegmentInvariantsCache` (Region A RaBitQ) AND `SurvivorRangeCache` (Region B SQ8) get the same treatment. At 1M scale, the DRAM budget (256MB invariants + 1024MB survivor) covers everything. At billion-scale, DRAM can't hold all segments → the local-disk tier catches the overflow → near-zero object-store GETs.

## What Already Exists (audit before reuse)

1. **`NvmeBackend<K,V>`** — `src/storage/cache/backend/nvme_tier.rs`.
   Despite its name and shard-directory setup, values live only in a DashMap.
   It is not the persistent implementation used by this design.

2. **L1→L2 spillover pattern** — `src/storage/cache/base.rs`:
   - `spill_to_l2(key, value)` — called on L1 eviction.
   - `promote_to_l1(key, value)` — called on L2 hit.
   - **The pattern exists, just not connected to the warm-tier caches.**

3. **`TenantCache<V>`** — `crates/foundation/proximadb-cache/src/lib.rs`. The moka-backed, byte-budgeted, per-tenant fair-sharing cache that backs `SurvivorRangeCache`. Has:
   - `get_or_load` read-through API with loader closure.
   - Eviction listener (for gauge updates).
   - Per-kind admission ceilings (`CacheKind::QuantizedCodes` etc.).
   - Per-tenant weighted fair share + true-pin reserves.

4. **`SegmentInvariantsCache`** — `src/storage/engines/sst/segment_format.rs:910-1149`. Custom sharded HashMap (16 shards, RWLocks). Priority-then-recency eviction (InvariantIndex pinned hardest, priority=3).

5. **`CacheTier` enum** — `src/storage/engines/sst/segment_format.rs`:
   ```rust
   InvariantIndex  // Region A RaBitQ — priority 3 (never evict from DRAM)
   SearchControl   // Header/A0 — priority 3
   InvariantMeta   // Footer — priority 2
   ProbeIndex     // IVF cell selections — priority 0
   SurvivorPayload // Region B SQ8 — priority 0 (evict first from DRAM)
   ResultPayload   // Region D OIDs — priority 1
   ```

6. **`SurvivorRangeCache` header doc** identified a persistent local spill tier
   as the 10M+ follow-up. This is that follow-up, with the medium generalized
   from NVMe to local disk.

## Implementation audit correction (2026-07-29)

First-principles inspection changed one implementation assumption without
changing the requested D1-D6 outcome:

- The root `NvmeBackend` is **not file-backed**. It creates shard directories
  but keeps values in a `DashMap`, cannot rebuild after restart, and estimates
  bytes from `Debug` output. Wiring it would fail this handoff's second-restart
  test. The real persistent byte store therefore lives in the foundation
  `proximadb-cache` crate (the root crate cannot be imported from a foundation
  crate without a layering cycle). See ADR-085.
- Local range reads validate persisted CRCs per 4 MiB chunk. Reading and
  checksumming an entire 128 MiB SQ8 parent for a small survivor range would
  remove Blob GETs but introduce avoidable local read amplification.
- Write-time population is **zero extra object I/O/encoding**, not literally
  zero cost: it retains memory and performs hashing, checksumming, and an
  optional local write. It becomes visible only after final atomic publication.
- "Free NVMe" means no separately metered managed-disk line item on an
  instance-store SKU. Its capacity is bundled into the selected VM and is not
  economically or operationally free in isolation.
- All impact rows below remain projections until a corrected benchmark proves
  quiescent materialization, segment geometry, cache state, physical GET count,
  and recall and records the result in `BENCHMARK_EVIDENCE.toml`.

## What to Build

### D1 — Wire local-disk L2 into `TenantCache` (backs SurvivorRangeCache)

**File:** `crates/foundation/proximadb-cache/src/lib.rs`

Add `l2_backend: Option<Arc<NvmeBackend<CacheKey, V>>>` field.

In `get_or_load`:
1. L1 (DRAM/moka) hit → return.
2. L1 miss → **check local-disk L2** → on hit, promote to L1 (admission-controlled), return.
3. L2 miss → run the loader (object store fetch) → insert into L1 + L2.

In moka eviction listener:
- On eviction → **spill to L2**.

### D2 — Wire local-disk L2 into `SegmentInvariantsCache`

**File:** `src/storage/engines/sst/segment_format.rs`

The InvariantsCache uses a custom sharded HashMap (not TenantCache). Add an optional persistent byte-store handle alongside each shard. On shard miss → check local disk → on hit, promote to shard. On shard eviction (LRU) → spill to local disk.

The key: `{segment_path}:{region_kind}` (e.g., `"data/.../seg.pax:RaBitQ"` vs `"data/.../seg.pax:Footer"`).

### D3 — Shared local-disk path + per-kind budgets

Both caches share ONE local directory (e.g., `/var/lib/proximadb/cache/`). The persistent store's sharded directory structure separates entries by stable key hash. Priority eviction ensures survivor data does not crowd invariants out of the shared budget:

```toml
[storage.sst_config]
survivor_cache_mb = 1024                    # DRAM budget (existing)
segment_invariants_cache_mb = 256            # DRAM budget (existing)
cache_local_disk_path = "/var/lib/proximadb" # NEW: shared local path
cache_local_disk_max_gb = 100                 # NEW: shared disk budget
```

Env overrides:
- `PROXIMADB_CACHE_LOCAL_DISK_PATH` — the local-filesystem path.
- `PROXIMADB_CACHE_LOCAL_DISK_MAX_GB` — the shared budget.
- Unset → no L2 (current behavior, unchanged, zero regression).

### D4 — Metrics

Add Prometheus counters:
- `proximadb_cache_local_disk_hits_total{tier="survivor|invariants"}` — L2 hit (0 GETs).
- `proximadb_cache_local_disk_misses_total{tier="survivor|invariants"}` — L2 namespace miss.
- `proximadb_cache_local_disk_bytes{tier="survivor|invariants"}` — resident local bytes.

### D5 — Register env gates

Add `PROXIMADB_CACHE_LOCAL_DISK_PATH` + `PROXIMADB_CACHE_LOCAL_DISK_MAX_GB` to `docs/12-design/ENV_GATE_REGISTRY.adoc`.

## Scale Analysis (why both caches need local disk)

| Scale | RaBitQ (A) per segment | SQ8 (B) per segment | Segments | DRAM needs | Local-disk needs |
|---|---|---|---|---|---|
| 1M | 24 MB | 128 MB | 2 | 304 MB ✅ | 0 (fits DRAM) |
| 10M | 24 MB | 128 MB | ~5 | 760 MB ⚠️ | 0-2 GB (overflow) |
| 100M | 24 MB | 128 MB | ~20 | 3 GB ❌ | ~15 GB |
| 1B | 24 MB | 128 MB | ~100 | 15 GB ❌ | ~100 GB |

At billion-scale with a 4GB DRAM instance, the local-disk budget retains the
immutable working set that does not fit in DRAM. The exact split depends on
segment geometry, metadata size, disk budget, and eviction history; it must be
measured rather than inferred solely from corpus size.

## Expected Impact on GETs/query

| Scenario | DRAM hit% | Local-disk hit% | Object store% | GETs/query | Retrieval COGS/M | Margin @ $50/M |
|---|---|---|---|---|---|---|
| Current (DRAM only, 1M cold) | 32% | 0% | 68% | 48.5 | $24.25 | 51.5% |
| DRAM + local disk (1M warm) | 32% | **60%** | 8% | **~5.8** | **$2.9** | **94%** |
| DRAM + local disk (100M warm) | 15% | **75%** | 10% | **~10** | **$5.0** | **90%** |
| DRAM + local disk (1B warm) | 5% | **80%** | 15% | **~15** | **$7.5** | **85%** |

### Attribution — survivors dominate the reduction, not invariants

A common misread of this table: "cache the invariants (Region A + control
plane) and GETs drop ~10×." That overstates the invariants tier. Decomposing the
measured 48.5 GETs/query (2 segments) by PAX region — the 48.5 total is
*measured*; the per-region split is *model-derived (approximate)*, traced from
`rabitq_search_segment_coalesced` in `segment_format.rs`:

| Region | What | Covering cache | ~GETs/q (2 segs) | Share |
|---|---|---|---|---|
| Control plane | header + A0 + RaBitQ-hdr + footer | SegmentInvariantsCache | ~8–13 | ~20–25% |
| Region A probed cells | RaBitQ, nprobe≈14 cells | SurvivorRangeCache (`Other`) | ~10–16 | ~25% |
| **Region B SQ8 survivors** | the rerank payload | SurvivorRangeCache (`QuantizedCodes`) | **~16–24** | **~40–50%** |
| Region D OIDs | top-k metadata blocks | SurvivorRangeCache (`Other`) | ~2–4 | ~5% |

The 48.5 → ~5.8 projection is driven by the **survivor cache + local disk** (~40–50%
of GETs), *not* invariants (~20–25%). Invariants caching alone takes 48.5 → ~38.
The full warm-tier stack (both caches + local disk) is what reaches ~5.8. All six PAX
regions route through one of the two warm-tier caches, so the local-disk plan (D1 into
`TenantCache`→SurvivorRangeCache, D2 into SegmentInvariantsCache) covers the
entire read path — including Region D OIDs
(`survivor_cache.get_or_fetch(CacheKind::Other, …)`, `segment_format.rs:2111`).

## Prerequisite — fix the 0% SegmentInvariantsCache hit (blocks D2)

The `level_multiplier=1.0` benchmark measured the invariants cache at **0% hit**
(vs 99.96% in the earlier Phase-2 run). This is not a side note — **it gates
D2.** Local disk sits *behind* the invariants cache: on an L1 miss the code
checks L2. Both authoritative read-through and post-publication write seeding
populate L2, but an L1 population bug would still hide the intended DRAM
benefit. Fix that defect before declaring D2 complete.

Code trace (`segment_format.rs`, `core.rs:531`):
- The cache **is armed** (process-wide `OnceLock`, `Some` when
  `segment_invariants_cache_mb > 0`). Not an arming bug.
- Lookup `c.get(path)` and insert `c.put(path, …)` use the same key. Not a key
  mismatch.
- The insert is gated by a `cache_changed` flag that is **false on the
  coarse-probe path** unless `a0_bytes`/`rabitq_header_bytes` were newly filled.
  Leading hypothesis: on probe-armed segments the insert conditions don't fire →
  entries never populate → 0% hit.
- **Needs an instrumented re-run to confirm** (`PROXIMADB_TRACE_GETS` + the
  `SEGMENT_INVARIANTS_CACHE_HITS/MISSES_TOTAL` counters). Code-reading says
  "should hit"; the measurement says "doesn't" — close the gap with a
  measurement, not more reading.

Acceptance: invariants-cache hit rate **≥ 90% on warm repeat queries at 1M**
(measured via the Prometheus counters) before D2 is considered done.

## Azure Economics (why this beats Turbopuffer)

| Component | Turbopuffer (AWS) | ProximaDB (Azure) |
|---|---|---|
| Object storage | $0.023/GB-mo (S3) | $0.018/GB-mo (Blob Hot) |
| SSD cache layer | **$0.10/GB-mo** (reported competitor price) | bundled with selected VM when using instance-store NVMe; managed-disk cost otherwise |
| DRAM cache | Included in compute | Included in compute |
| Total storage+cache | $0.12/GB-mo | $0.018/GB-mo (**6.7× cheaper**) |
| Queries (cold) | ~$1/PB (S3 GETs ~$0.004/10K) | $50/M flat (Blob GETs ~$0.005/10K) |
| Queries (local-disk warm) | no object-store GET fee | no object-store GET fee; local media/compute still costs |

At 100M+ scale, the storage markup dominates: Turbopuffer charges $0.10/GB for SSD cache that ProximaDB includes for free. ProximaDB undercuts at ~2× cheaper.

## Multitenancy

The existing `TenantCache` per-tenant fair-sharing (TD-CACHE-3: floors, ceilings, true-pin reserves, work-conserving elastic budget) continues to govern DRAM admission. The local-disk key includes the tenant identity, preventing cross-tenant key collisions; its shared capacity policy is priority/recency rather than the DRAM tier's per-tenant quota scheme. Shared multi-tenant is the default; BYOC is an Enterprise option.

## Key Files

| File | Role |
|---|---|
| `crates/foundation/proximadb-cache/src/lib.rs` | `TenantCache` — add L2 backend |
| `crates/foundation/proximadb-cache/src/persistent_l2.rs` | `PersistentByteStore` — restart-persistent local store |
| `src/storage/cache/backend/nvme_tier.rs` | Legacy `NvmeBackend` — audited only; memory-backed, not reused |
| `src/storage/cache/base.rs` | L1→L2 spillover pattern — existing, reference |
| `src/storage/engines/sst/survivor_range_cache.rs` | `SurvivorRangeCache` — pass local store to TenantCache |
| `src/storage/engines/sst/segment_format.rs` | `SegmentInvariantsCache` — add local-disk L2 lookup on miss |
| `src/storage/engines/sst/core.rs` | `SstEngine::new` — wire local-disk path from config |
| `src/core/config.rs` | `SstConfig` — add `cache_local_disk_path` + `cache_local_disk_max_gb` |
| `src/metrics/operational_metrics.rs` | Add local-disk hit/miss/bytes counters |
| `docs/12-design/ENV_GATE_REGISTRY.adoc` | Register two new env gates |

## Design Docs (already merged to develop — read these first)

- `docs/12-design/adr/ADR-080-vector-search-compaction-tuning.adoc` — the L2 compaction fix (level_multiplier=1.0) + the verified 48.5 GETs measurement.
- `docs/10-quality/td/TD-COMPACT-10-l2-segment-consolidation-gap.adoc` — the measured segment-count problem + verified fix.
- `docs/10-quality/td/TD-SEARCH-3-per-operation-read-coalescing.adoc` — Azure charges per GET (not per byte) + the coalescing opportunity.
- `docs/12-design/adr/ADR-065-*.adoc` — the PAX region layout + the original warm-tier cache design.
- `docs/12-design/HANDOFF_L2_COMPACTION_GET_REDUCTION.md` — the original Codex handoff for the L2 compaction fix.

## Next reductions beyond caching (the post-local-disk levers)

Once D1–D6 land, the warm-tier seam is complete — **caching is near-maxed**.
The next 2–3× comes from model quality and I/O coalescing, not more cache tiers:

1. **Per-operation coalescing (TD-SEARCH-3, designed not built).** Azure bills
   per *operation* — a 72B GET and a 16KB GET cost the same ($0.005/10K).
   Coalesce header+A0 into one GET, footer+last-SQ8-range into one GET.
   ~2–4 GETs/query cold. Independent of caching; stacks on top of it.
2. **nprobe reduction — the multiplicative lever.** `nprobe = √k_c ×
   multiplier`; at 2 large segments it's ~14 each. Region A *and* Region B reads
   both scale with nprobe, so **halving nprobe cuts ~25 GETs/query** — bigger
   than the entire invariants tier. Model-quality lever (better IVF training →
   fewer cells for same recall). Deepest lever; compounds with everything.
   Candidate TD-SEARCH-4.
3. **Concurrent-read dedup.** N queries probing one segment each re-read Region
   A. An in-flight `read_range` coalescer dedups identical concurrent reads —
   marginal in a single-query bench, real under production load.

Caching gets GETs/query to ~5–15 depending on scale. Coalescing + nprobe is the
path to <5.

## Acceptance Criteria

0. **(Prerequisite, blocks D2)** SegmentInvariantsCache hit rate ≥ 90% on warm
   repeat queries at 1M (Prometheus `SEGMENT_INVARIANTS_CACHE_HITS_TOTAL` /
   misses). Fix the measured 0% hit *before* declaring local-disk promotion complete — otherwise
   D2 is dead weight. See "Prerequisite" section above.
1. Local-disk L2 is optional (env-gated, unset = current behavior, zero regression).
2. Both `SurvivorRangeCache` AND `SegmentInvariantsCache` retain immutable values on local disk.
3. Both promote from local disk to DRAM on L2 hit.
4. At 1M SIFT (warm local disk after one cold sweep): GETs/query ≤ **10** (vs 48.5 cold).
5. Recall@10 ≥ 0.98 maintained (same data, faster access).
6. Per-tenant local-disk key isolation (cache key includes tenant).
7. Metrics: `proximadb_cache_local_disk_hits_total`, `_misses_total`, `_bytes` with tier labels.
8. `cargo clippy --lib --bins -D warnings` clean.
9. `make work-commit-check` clean (including env-gate registry).

## How to Verify

The authoritative implementation harness is now
`scripts/bench/sift1m_get_reduction.py`. It builds on the sequence below but
adds release-binary enforcement, full PAX-footer row accounting, a stable
segment-geometry gate, persisted collection identity, exact metric presence,
fresh-process cache states, and machine-readable output. The following remains
the original manual sketch, not evidence protocol:

```bash
# 1. Boot with the persistent local-disk tier (NVMe is recommended in production)
PROXIMADB_CACHE_LOCAL_DISK_PATH=/tmp/proximadb_local_cache \
PROXIMADB_CACHE_LOCAL_DISK_MAX_GB=10 \
PROXIMADB_L0_COMPACTION_ENABLED=1 \
  target/release-server/proximadb-server -c config.toml

# 2. Ingest 1M SIFT + settle
python3 scripts/bench/async_compaction_1m_verify.py
# Wait 120s for compaction

# 3. Cold sweep #1 (populates DRAM + local disk)
# Restart server (cold DRAM, cold local disk)
pkill proximadb-server; sleep 3; <boot>; sleep 20
python3 scripts/bench/cache_hot_vs_cold_1m.py
# Record: GETs/query (expect ~48 cold), local-disk hits = 0

# 4. Restart again (cold DRAM, but local cache persists)
pkill proximadb-server; sleep 3; <boot>; sleep 20
python3 scripts/bench/cache_hot_vs_cold_1m.py
# Record: GETs/query (expect ~5-10 — local disk catches the overflow)
# Scrape: proximadb_cache_local_disk_hits_total > 0
```

## D6 — Write-Time Cache Population (policy-driven)

### The insight: the writer ALREADY has the data in DRAM

During `write_pax_segment_compacted` (`segment_format.rs`), the writer has the
RaBitQ codes (24MB) and SQ8 codes (128MB) **in memory** — it's encoding them
INTO the segment file. Inserting them into the cache is an `Arc<[u8]>` clone +
hash insert. **Zero extra I/O. Zero extra GETs. Nanoseconds of CPU.**

```
CURRENT write path:
  vectors → encode RaBitQ → encode SQ8 → write .pax file → DROP data → done
  First query: COLD MISS → GET from object store → populate cache (expensive)

PROPOSED write path:
  vectors → encode RaBitQ → encode SQ8 → write .pax file → INSERT into cache → done
  First query: CACHE HIT → 0 GETs (free!)
```

### Policy

```toml
[storage.sst_config]
# Write-time cache population policy
# "none" = lazy read-through (current behavior, zero regression)
# "invariant" = cache RaBitQ Region A + header/A0/footer on write (default)
# "all" = cache RaBitQ + SQ8 on write
cache_on_write = "invariant"
```

| Policy | Write-time cost | Cached on write | First-query GETs saved | Budget impact |
|---|---|---|---|---|
| `none` | 0 | nothing | 0 | none |
| `invariant` | ~0 (Arc clone) | RaBitQ (24MB) + header/A0/footer | ~4/segment | 48MB at 2 segments — fits 256MB |
| `all` | ~0 (Arc clone) | RaBitQ + SQ8 | ~7/segment | 304MB at 2 segments — fits 1280MB combined |

**Recommended default: `invariant`** — near-zero cost, eliminates the 0% invariants
cache hit rate observed in the benchmark (4 GETs/segment wasted on cold first query).

### Why this is free (co-design first principles)

The data being cached is ALREADY in DRAM during the write — the writer just
encoded it. The insert is:
1. `Arc::from(rabitq_bytes)` — one allocation (or zero if already Arc).
2. `cache.insert(key, arc_value)` — one hash insert.

No I/O. No GET. No extra encode. The bytes are dropped after the write anyway —
this just keeps them alive via the cache's Arc reference.

### Budget pressure handling

The cache is byte-budgeted (256MB invariants + 1024MB survivor). When write-time
insertion exceeds the budget, LRU eviction drops the OLDEST entries. This
**naturally prioritizes fresh data** — the most recently written segments are the
most likely to be queried next. No special logic needed.

### What does NOT get warmed on restart

Write-time caching only applies to **NEW writes** (post-boot). For previously-
written segments:

1. **Hot data** → warmed via TD-CACHE-1 manifest (`src/storage/engines/sst/warming.rs`).
   The existing mechanism: shutdown emits top-K hot keys; boot replays them. This
   is demand-driven (only recently-queried ranges) — NOT a full sweep.

2. **Cold data** → lazy read-through on first query (current behavior).

**Re-reading ALL data on restart would waste GETs** — the user's instinct is
correct. The existing manifest mechanism is the right hybrid:
- Write-time caching for fresh writes (free, in-DRAM during write).
- Manifest warming for hot data (a few GETs for top-K ranges).
- Lazy read-through for the cold tail (1 GET on first access).

### Implementation (in write_pax_segment_compacted)

In `src/storage/engines/sst/segment_format.rs`, after writing each region to the
file, check the policy and insert into the cache:

```rust
// After writing RaBitQ (Region A) bytes to the file:
if policy >= CacheOnWritePolicy::Invariant {
    if let Some(inv_cache) = invariants_cache {
        inv_cache.insert(
            &segment_path,
            CacheTier::InvariantIndex,
            Arc::from(rabitq_bytes.as_slice()),
        );
    }
}

// After writing SQ8 (Region B) bytes:
if policy >= CacheOnWritePolicy::All {
    if let Some(surv_cache) = survivor_cache {
        surv_cache.insert(
            CacheKind::QuantizedCodes,
            &segment_path,
            sq8_off,
            sq8_len,
            Arc::from(sq8_bytes.as_slice()),
        );
    }
}
```

The `rabitq_bytes` / `sq8_bytes` are already in scope — the writer just encoded
them for the file write. No extra work.

### Files to modify (additional to D1-D5)

- `src/storage/engines/sst/segment_format.rs` — `write_pax_segment_compacted` +
  `write_pax_segment_full`: insert into caches after writing each region.
- `src/core/config.rs` — add `cache_on_write: CacheOnWritePolicy` to `SstConfig`.
- `src/storage/engines/sst/core.rs` — pass the policy + cache handles through to
  the writer.

### Combined GET-reduction stack

| Lever | When it fires | GETs saved | Cost |
|---|---|---|---|
| **Write-time caching** (D6) | On write/compaction | ~4-7 per segment (first query) | ~0 (Arc clone) |
| **Local-disk L2 retention** (D1-D2) | On authoritative load/write | All subsequent reads of retained data | local read |
| **Manifest warming** (TD-CACHE-1, existing) | On restart | Top-K hot ranges | Few GETs |
| **Lazy read-through** (existing) | First query on cold data | N/A (the fallback) | 1 GET per access |

**Together:** the ONLY time a GET happens is for data that was NEVER written
(cold new collections) or never queried and evicted from the configured
local-disk budget.

### Expected combined impact

| Scenario | Without D6 | With D6 (write-time + local disk) |
|---|---|---|
| First query after write | 48.5 GETs (cold) | **~0 GETs** (write-time cached) |
| Second query (DRAM warm) | ~0 GETs | ~0 GETs |
| After DRAM eviction + restart | ~48.5 GETs (cold) | **~5-10 GETs** (local-disk L2 hit) |
| After local-disk eviction | ~48.5 GETs (cold) | ~48.5 GETs (fallback to object store) |

## Projected rate-card economics (why this ships — pricing rationale)

The local-disk tier + write-time caching are not just perf work — they are a **pricing
prerequisite**. Modeling the current rate card (KSU $0.02/GB-mo, KRU $50/M flat,
KIU $0.75/GB) against the verified GET measurements shows the cold path goes
negative at scale, but the blended path stays strongly profitable everywhere.

=== Headline

**The $50/M KRU flat rate is safe AFTER the local-disk tier ships.** On the cold path
alone it goes negative at ≥100M. On the blended path (the realistic steady state
once D1–D6 land) it holds 61–90% margin at every scale up to 1B.

Blend assumption (the realistic steady state post-D6): 50% write-time-hot
(0 GETs) + 35% local-disk-warm + 15% cold. GETs at each scale are projected from the
verified 48.5 cold at 1M, scaled by segment count (each segment adds ~7-8 reads,
nprobe scales as sqrt(k_c)).

=== Per-scale margin (Azure Hot tier, $0.005/10K reads)

[cols="2,1,1,1,1,1"]
|===
| Scale | Cold GETs/q | Cold margin | Blended GETs/q | Blended margin | vs Turbopuffer $16 floor

| 1M   | 48.5  | **+51.5%** | 9.7  | +90.3% | WIN ($7.27)
| 10M  | 72    | **+28.0%** | 15.0 | +85.0% | WIN ($11.35)
| 100M | 120   | **−20%**   | 24.3 | +75.7% | tie ($19.66 ≈ $16)
| 1B   | 200   | **−100%**  | 38.8 | +61.3% | WIN ($44.20 vs $61.48)
|===

The cold-path negatives at 100M/1B are the reason the tier must ship before we
price large corpora at $50/M flat. Until then the rate card carries a note:
"assumes ≥35% blended cache hit (default with local-disk + write-time caching)."

=== Rate-card decisions

. **KRU $50/M flat — KEEP.** Designed for 36–51 GETs (anvaiops ADR-0044).
  Measured 48.5 cold at 1M is inside the design band → 51.5% cold margin. Blended
  pushes it to 85–90%. Do not lower it.

. **KSU $0.02/GB-mo — KEEP, and make the bundled local tier the headline.** Storage
  margin is 82% (engine bytes × $0.018 vs raw × $0.020). Turbopuffer charges
  $0.10/GB for the SSD cache layer we include free → **6.7× cheaper
  storage+cache stack**. This compounds at scale; the query rate does not.

. **Do NOT chase S3 Vectors on per-query price.** $2.53/mo at 1M is AWS's
  loss-leader bare ANN — structurally unwinnable with a flat query rate, and
  AWS-only. The moat is Azure-native + managed + free tier + multitenancy.

. **Turbopuffer $16 floor is the real win-line.** Beatable at ≤10M blended,
  ties at 100M, wins decisively at 1B (where its storage markup dominates).

=== Competitor map (1M queries/month, blended 33% margin)

|===
| Offering | Monthly @1M | Monthly @1B | Notes

| **ProximaDB** (blended) | $7.27 | $44.20 | local tier on bundled instance NVMe
| Turbopuffer (AWS) | $16.00 (floor) | $61.48 | BYOC; $0.10/GB SSD markup
| S3 Vectors (AWS) | $2.53 | $33.22 | bare ANN, AWS-only
| Qdrant (Azure VM) | $100+ | $100+ | self-managed
| Pinecone | $250+ | $250+ | managed, premium
|===

=== unit_economics.json corrections (the source is stale)

Wherever the rate card sources its COGS assumptions, four values drifted and must
be corrected when the code lands:

[cols="3,2,2,3"]
|===
| Field | Current | Corrected | Reason

| `read_request_usd_per_10k.azure` | 0.0065 | **0.005** | web-verified Azure Hot tier
| `measured_cold_gets_per_query_at_gate` | 51 | **48.5** | post level_multiplier=1.0
| `pooled_vector_limit` | 5,000,000 | **100,000,000** | post local-disk tier (1B needs quote)
| `minimum_cache_hit_ratio_at_gate` | 0.5 | **0.3** | local-disk tier makes this easier
|===
