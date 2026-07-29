# Handoff: NVMe Tier Promotion — DRAM→NVMe Spill for Both Cache Tiers

## Context

The SIFT1M end-to-end benchmark (2026-07-29) measured **48.5 GETs/query cold** at 1M scale (2 segments, `level_multiplier=1.0`). The dominant cost driver is SQ8 survivor range misses (38.5 GETs/query at 32% DRAM cache hit) + control-plane misses (4 GETs/query at 0% invariants cache hit — a separate bug). Each cold miss = 1 Azure Blob GET ($0.005/10K).

**Azure instance store NVMe is free** (bundled in Lsv2/E-series VM price). Turbopuffer charges $0.10/GB for this same SSD caching. ProximaDB can include it for free.

**The `NvmeBackend` + L1→L2 spillover pattern already exist in the codebase** (`src/storage/cache/backend/nvme_tier.rs`, `src/storage/cache/base.rs`) — but are NOT wired into either the SurvivorRangeCache or the SegmentInvariantsCache.

## The Goal

Wire the existing NVMe tier machinery into BOTH warm-tier caches:

```
Object Store (Azure Blob, $0.005/10K GETs)
    ↑ cold miss (first-ever access to a segment)
    |
NVMe L2 (instance store — FREE on Azure Lsv2)
    ↑ warm miss (evicted from DRAM, still on NVMe)
    |
DRAM L1 (moka — admission-controlled by CacheTier priority)
    = hot hit (the working set)
```

Both `SegmentInvariantsCache` (Region A RaBitQ) AND `SurvivorRangeCache` (Region B SQ8) get the same treatment. At 1M scale, the DRAM budget (256MB invariants + 1024MB survivor) covers everything. At billion-scale, DRAM can't hold all segments → the NVMe tier catches the overflow → near-zero object-store GETs.

## What Already Exists (reuse, don't rebuild)

1. **`NvmeBackend<K,V>`** — `src/storage/cache/backend/nvme_tier.rs`. Full implementation: sharded dirs (16 shards), DashMap index, LRU eviction, size tracking. Implements `StorageBackend` trait. **Tested.**

2. **L1→L2 spillover pattern** — `src/storage/cache/base.rs`:
   - `spill_to_l2(key, value)` — called on L1 eviction.
   - `promote_to_l1(key, value)` — called on L2 hit.
   - **The pattern exists, just not connected to the warm-tier caches.**

3. **`TenantCache<V>`** — `crates/foundation/proximadb-cache/src/lib.rs`. The moka-backed, byte-budgeted, per-tenant fair-sharing cache that backs `SurvivorRangeCache`. Has:
   - `get_or_load` read-through API with loader closure.
   - Eviction listener (for gauge updates — extend for NVMe spill).
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

6. **`SurvivorRangeCache` header doc** explicitly says: *"NVMe spill for 10M+ scale swaps the backing behind the same key/budget API (follow-up)."* — this IS the designed follow-up.

## What to Build

### D1 — Wire NVMe L2 into `TenantCache` (backs SurvivorRangeCache)

**File:** `crates/foundation/proximadb-cache/src/lib.rs`

Add `l2_backend: Option<Arc<NvmeBackend<CacheKey, V>>>` field.

In `get_or_load`:
1. L1 (DRAM/moka) hit → return.
2. L1 miss → **check L2 (NVMe)** → on hit, promote to L1 (admission-controlled), return.
3. L2 miss → run the loader (object store fetch) → insert into L1 + L2.

In moka eviction listener:
- On eviction → **spill to L2** (fire-and-forget `tokio::spawn`).

### D2 — Wire NVMe L2 into `SegmentInvariantsCache`

**File:** `src/storage/engines/sst/segment_format.rs`

The InvariantsCache uses a custom sharded HashMap (not TenantCache). Add an optional `Arc<NvmeBackend<...>>` alongside each shard. On shard miss → check NVMe → on hit, promote to shard. On shard eviction (LRU) → spill to NVMe.

The key: `{segment_path}:{region_kind}` (e.g., `"data/.../seg.pax:RaBitQ"` vs `"data/.../seg.pax:Footer"`).

### D3 — Shared NVMe path + per-kind budgets

Both caches share ONE NVMe directory (e.g., `/var/lib/proximadb/cache/`). The `NvmeBackend`'s sharded directory structure naturally separates entries by key hash. Per-kind budgets ensure invariants don't crowd out survivors on NVMe:

```toml
[storage.sst_config]
survivor_cache_mb = 1024                    # DRAM budget (existing)
segment_invariants_cache_mb = 256            # DRAM budget (existing)
cache_nvme_path = "/var/lib/proximadb"      # NEW: shared NVMe path
cache_nvme_max_gb = 100                      # NEW: shared NVMe budget
```

Env overrides:
- `PROXIMADB_CACHE_NVME_PATH` — the instance-store path.
- `PROXIMADB_CACHE_NVME_MAX_GB` — the shared budget.
- Unset → no L2 (current behavior, unchanged, zero regression).

### D4 — Metrics

Add Prometheus counters:
- `proximadb_cache_nvme_hits_total{tier="survivor|invariants"}` — L2 hit (0 GETs).
- `proximadb_cache_nvme_misses_total{tier="survivor|invariants"}` — L2 miss (object store).
- `proximadb_cache_nvme_bytes{tier="survivor|invariants"}` — resident NVMe bytes.

### D5 — Register env gates

Add `PROXIMADB_CACHE_NVME_PATH` + `PROXIMADB_CACHE_NVME_MAX_GB` to `docs/12-design/ENV_GATE_REGISTRY.adoc`.

## Scale Analysis (why both caches need NVMe)

| Scale | RaBitQ (A) per segment | SQ8 (B) per segment | Segments | DRAM needs | NVMe needs |
|---|---|---|---|---|---|
| 1M | 24 MB | 128 MB | 2 | 304 MB ✅ | 0 (fits DRAM) |
| 10M | 24 MB | 128 MB | ~5 | 760 MB ⚠️ | 0-2 GB (overflow) |
| 100M | 24 MB | 128 MB | ~20 | 3 GB ❌ | ~15 GB |
| 1B | 24 MB | 128 MB | ~100 | 15 GB ❌ | ~100 GB |

At billion-scale with a 4GB DRAM instance: DRAM holds ~20 segments' invariants + ~10 segments' survivors. NVMe holds the remaining 80 segments. Object store is only for never-queried segments (the cold tail).

## Expected Impact on GETs/query

| Scenario | DRAM hit% | NVMe hit% | Object store% | GETs/query | Retrieval COGS/M | Margin @ $50/M |
|---|---|---|---|---|---|---|
| Current (DRAM only, 1M cold) | 32% | 0% | 68% | 48.5 | $24.25 | 51.5% |
| DRAM + NVMe (1M warm) | 32% | **60%** | 8% | **~5.8** | **$2.9** | **94%** |
| DRAM + NVMe (100M warm) | 15% | **75%** | 10% | **~10** | **$5.0** | **90%** |
| DRAM + NVMe (1B warm) | 5% | **80%** | 15% | **~15** | **$7.5** | **85%** |

## Azure Economics (why this beats Turbopuffer)

| Component | Turbopuffer (AWS) | ProximaDB (Azure) |
|---|---|---|
| Object storage | $0.023/GB-mo (S3) | $0.018/GB-mo (Blob Hot) |
| SSD cache layer | **$0.10/GB-mo** (EC2 instance store markup) | **$0** (instance store NVMe — FREE) |
| DRAM cache | Included in compute | Included in compute |
| Total storage+cache | $0.12/GB-mo | $0.018/GB-mo (**6.7× cheaper**) |
| Queries (cold) | ~$1/PB (S3 GETs ~$0.004/10K) | $50/M flat (Blob GETs ~$0.005/10K) |
| Queries (warm NVMe) | ~$0 (cached on SSD) | ~$0 (cached on NVMe — same model) |

At 100M+ scale, the storage markup dominates: Turbopuffer charges $0.10/GB for SSD cache that ProximaDB includes for free. ProximaDB undercuts at ~2× cheaper.

## Multitenancy

The existing `TenantCache` per-tenant fair-sharing (TD-CACHE-3: floors, ceilings, true-pin reserves, work-conserving elastic budget) applies to the NVMe tier automatically — the `CacheKey` includes `tenant: Arc<str>`, and the `NvmeBackend` hashes the full key. No cross-tenant contention. Shared multi-tenant is the default; BYOC is an Enterprise option.

## Key Files

| File | Role |
|---|---|
| `crates/foundation/proximadb-cache/src/lib.rs` | `TenantCache` — add L2 backend |
| `src/storage/cache/backend/nvme_tier.rs` | `NvmeBackend` — existing, reuse as-is |
| `src/storage/cache/base.rs` | L1→L2 spillover pattern — existing, reference |
| `src/storage/engines/sst/survivor_range_cache.rs` | `SurvivorRangeCache` — pass NVMe config to TenantCache |
| `src/storage/engines/sst/segment_format.rs` | `SegmentInvariantsCache` — add NVMe L2 lookup on miss |
| `src/storage/engines/sst/core.rs` | `SstEngine::new` — wire NVMe path from config |
| `src/core/config.rs` | `SstConfig` — add `cache_nvme_path` + `cache_nvme_max_gb` |
| `src/metrics/operational_metrics.rs` | Add NVMe hit/miss/bytes counters |
| `docs/12-design/ENV_GATE_REGISTRY.adoc` | Register two new env gates |

## Design Docs (already merged to develop — read these first)

- `docs/12-design/adr/ADR-080-vector-search-compaction-tuning.adoc` — the L2 compaction fix (level_multiplier=1.0) + the verified 48.5 GETs measurement.
- `docs/10-quality/td/TD-COMPACT-10-l2-segment-consolidation-gap.adoc` — the measured segment-count problem + verified fix.
- `docs/10-quality/td/TD-SEARCH-3-per-operation-read-coalescing.adoc` — Azure charges per GET (not per byte) + the coalescing opportunity.
- `docs/12-design/adr/ADR-065-*.adoc` — the PAX region layout + the warm-tier cache design (A in DRAM, B in DRAM/NVMe, D streamed).
- `docs/12-design/HANDOFF_L2_COMPACTION_GET_REDUCTION.md` — the original Codex handoff for the L2 compaction fix.

## Acceptance Criteria

1. NVMe L2 is optional (env-gated, unset = current behavior, zero regression).
2. Both `SurvivorRangeCache` AND `SegmentInvariantsCache` spill to NVMe on DRAM eviction.
3. Both promote from NVMe to DRAM on L2 hit.
4. At 1M SIFT (warm NVMe after one cold sweep): GETs/query ≤ **10** (vs 48.5 cold).
5. Recall@10 ≥ 0.98 maintained (same data, faster access).
6. Per-tenant NVMe isolation (TenantCache key includes tenant).
7. Metrics: `proximadb_cache_nvme_hits_total`, `_misses_total`, `_bytes` with tier labels.
8. `cargo clippy --lib --bins -D warnings` clean.
9. `make work-commit-check` clean (including env-gate registry).

## How to Verify

```bash
# 1. Boot with NVMe path on local disk (stand-in for instance store)
PROXIMADB_CACHE_NVME_PATH=/tmp/proximadb_nvme \
PROXIMADB_CACHE_NVME_MAX_GB=10 \
PROXIMADB_L0_COMPACTION_ENABLED=1 \
  target/release-server/proximadb-server -c config.toml

# 2. Ingest 1M SIFT + settle
python3 scripts/bench/async_compaction_1m_verify.py
# Wait 120s for compaction

# 3. Cold sweep #1 (populates DRAM + NVMe)
# Restart server (cold DRAM, cold NVMe)
pkill proximadb-server; sleep 3; <boot>; sleep 20
python3 scripts/bench/cache_hot_vs_cold_1m.py
# Record: GETs/query (expect ~48 cold), NVMe hits = 0

# 4. Restart again (cold DRAM, but NVMe PERSISTS on disk)
pkill proximadb-server; sleep 3; <boot>; sleep 20
python3 scripts/bench/cache_hot_vs_cold_1m.py
# Record: GETs/query (expect ~5-10 — NVMe catches the overflow)
# Scrape: proximadb_cache_nvme_hits_total > 0
```
