# I/O-trace co-design audit: object-store round-trip seams + depth-collapse

**Date:** 2026-06-28 · **Lens:** ADR-034 (object-storage-native; dominant cost = I/O **round-trips + egress**, not CPU) · **Instrument:** ADR-030 I/O trace (`src/observability/io_trace.rs`)

## What the trace measures (and its blind spot)

`IoTrace` records `get_ops`/`put_ops`/`list_ops`/`delete_ops`, `bytes_read`/`bytes_written`, `egress_bytes`, `footer_hits`/`footer_misses`, `compute_ms`, and `range_gets` (a **depth proxy** — count of ranged-GET requests). It is hooked at the **facade layer** (`record_store`, segment readers), so counts are *logical* ops.

**Blind spot:** the trace counts *depth* (serial GETs) but has **no dependent-vs-parallel distinction** — it can't tell K serial RTTs from K concurrent ones. That distinction is a query-planner/runtime concern. Existing evidence already indicts serial fan-out: `ROUTE_COST_OFFLINE_REGRET_2026_06_26.md` flags "DataFusion's per-partition GET fan-out dominates" (cost 220 vs 120).

## The co-design nuance (which lever each fix pulls)

For **distinct** objects, object stores have **no multi-object GET** — so two different optimizations exist, pulling different levers:

| Fix | Mechanism | Lever pulled |
|-----|-----------|--------------|
| **Concurrent batch** (`join_all` of K gets) | K requests issued together | **latency / depth** K→1 RTT — but op count stays K (still K GETs ⇒ KRU/$ unchanged) |
| **Segment batching** (K records in 1 object) | 1 GET returns K records | **op count** K→1 (the real KRU/$ win) — needs a storage-format change |
| **Range batching** (`get_ranges` within 1 object) | N ranges coalesced in 1 call | **depth** N→1 on the wire (object_store coalesces) |

So: concurrency buys latency; **op-count/$ reduction needs segment batching** (TD-168 residual #3 — now empirically justified, not just a write-amp cleanup).

## Ranked seams

| # | Seam | file:line | Cost | Status after this PR |
|---|------|-----------|------|----------------------|
| 1 | Graph BFS frontier materialization | `service_traversal_api.rs:453` | O(K) serial | **deferred** — uses engine-level `engine.get_node` (RAM-only, **cold-unaware**); needs cold-awareness *then* batching (see below) |
| 2 | Entity/search result materialization | `entity_service.rs:517` | O(M) serial `get_node` | **FIXED** — batched via `get_nodes` (depth M→1 for cold misses) |
| 3 | RecordStore had no batch get (root cause) | `store.rs:169` | forces #1/#2 loops | **FIXED** — `get_records(&[RecordKey])` added (default loop; concurrent override in `ColdGraphRecordStore`) |
| 4 | PAX block metadata assembly | `ranged_segment.rs:272` | up to 4 serial ranged GETs/block | **deferred** — batching the 3 metadata regions is a real 3→1 per-block win, but routing metadata through the *batched* bridge method collides with the TD-167 body-batch accounting invariant (`td167_pruned_read_batches_surviving_block_bodies_in_one_call` asserts exactly one batched bridge call). Needs the test reconciled (metadata vs body batches) — own change. |
| 5 | Segment-listing loop / footer HEAD+GET | `record_store.rs:3881` / `object_store.rs:309` | O(N) / 2-RTT | deferred (not per-query hot; footer cache mitigates) |

## What this PR changed

- **`RecordStore::get_records`** (foundation trait) — batch point-lookup, default loop, returns one slot per key in order. `ColdGraphRecordStore` overrides with `try_join_all` (concurrent object-store GETs → depth K→1).
- **`GraphOperationsService::get_nodes` / `get_edges`** — engine-hot lookups stay in RAM; engine misses are cold-fetched in **one concurrent batch**; cache admission unchanged from `cold_fetch_node`. Gate-OFF / no-store ⇒ engine-only (unchanged).
- **Entity search** (`entity_service`) materialization wired to `get_nodes` (seam #2).

(Seam #4, PAX metadata batching, was prototyped then **deferred** — see the table — because it collides with the TD-167 body-batch accounting test; it warrants its own change.)

All default-safe: no behavior change when the cold tier is OFF; `get_records` default impl keeps every existing `RecordStore` working unchanged.

## Deferred (with rationale)

- **Seam #1 (BFS frontier):** the engine traversal materializes via `engine.get_node` (RAM-only) — in cold mode it is **cold-unaware**, so it needs cold-awareness wired *before* batching helps. Larger change (engine ↔ service cold-fetch seam). The `get_nodes`/`get_records` primitives are now in place for it.
- **Op-count/$ reduction:** needs **TD-168 #3** (batch cold payloads into bounded segments + oid→segment index) — concurrency here only collapses latency.
- **PAX trait default `fetch_vector_segment_ranges`** still loops for test/in-memory bridges (the production `IcebergObjectStoreBridge` already overrides with `get_ranges`); low priority.
- **Suffix-range** (`get_suffix` HEAD+GET → 1) where the backend supports `Range: bytes=-N`; cache-mitigated, low priority.
