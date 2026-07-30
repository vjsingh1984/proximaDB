# Handoff: PR #1325 (feat/nvme-get-reduction-stack) — diagnosis + partial fixes for Codex

**From:** Claude session (took over #1325 after Codex credit exhaustion)
**Date:** 2026-07-30
**Status:** Partial fixes committed (storage CI failure + flush-plan regression resolved); the **graph/fusion CI failure is precisely diagnosed and handed off** — it is a deeper SST-engine regression on the native-search read path, most likely the new persistent-L2 integration.

## What #1325 is

`feat(storage): reduce GETs with persistent cache and admitted flushes` — the
get-reduction stack: persistent local-disk L2 for PAX invariant + survivor
caches (D1–D6), admitted flushes with typed WAL backpressure, higher-level
compaction consolidation, PAX probe-range coalescing, stable-id migration, and
a hardened SIFT bench harness. +6.5k lines, 65 files.

## CI state when handed back

Two CI jobs were failing: **Rust Integration Tests (storage)** and **Rust
Integration Tests (graph)**. I fixed the storage one and the flush-plan
regression; the **graph (fusion) failure remains** and is diagnosed below.

## What I FIXED (committed; build on or rework)

### 1. Storage tests — `tests/sst_compaction_execution_test.rs`, `tests/sst_ivf2_compaction_route_proof_test.rs`
The bare-`SstEngine` tests used **string** collection ids (`"wlp7_append"`,
etc.), but the PR's fail-closed `FlushParams::get_collection_object_id`
(`crates/storage/proximadb-storage-traits/src/types.rs`) requires a **decimal**
`CollectionObjectId`. Fixed the test `collection()` helpers with a catalog-style
numeric-id mock (`AtomicU64` minter). **4 storage tests now green** (incl. 2
latent `sst_ivf2` failures CI hadn't reached).

> **Rev2 note:** per ADR-0083 rev2 (composite-only), the test should ultimately
> key on the **composite** `CollectionIdentity`, not a numeric id. Rework when
> the rev2 migration lands; the mock is a bridge.

### 2. Flush-plan u64-parse — `crates/storage/proximadb-storage-traits/src/types.rs`, `crates/binding/proximadb-embedded/src/lib.rs`
The PR's `CollectionFlushPlan.collection_object_id = coll.id.parse::<u64>()`
errored because `Collection.id` holds a **UUID** (the embedded create path
doesn't mint a numeric object id). Fixed per **ADR-0083 rev2 D2** (the u64 is a
**derived, non-stored handle**, not a stored identity): `get_collection_object_id`
is now **parse-or-deterministic-hash** (`stable_collection_handle`, fixed-seed
`DefaultHasher`), and the embedded flush-plan construction derives the handle
likewise. Unblocks the flush (no longer errors on UUID); compiles clean.

> **Rev2 note:** this is the D2 "derived handle" bridge. The full migration
> (rev2) keys flush/admission on the **composite**; keep this as the derived
> handle or rework — your call.

## What REMAINS — the graph/fusion CI failure (diagnosed, handed off)

**Tests:** `embedded_code_graph_parity_e2e::embedded_fusion_search_seeds_and_expands`
+ `embedded_fusion_search_document_modality_contributes` — vector modality empty
(one returns 0 candidates; the other `source_count: 1` = Document only).

### Precise localization
- `db.search` (which uses **`VectorSearchRequest`**) **works** — `test_large_k`
  pre-reopen returns 1000/2000 vectors.
- Fusion uses **`unified_search_native_with_tenant_context`**
  (`src/services/operations/vectors/legacy.rs:2690`) → returns **`Ok([])`** for
  the SST engine (0 results, not an error — `retry_vector_search` *bails* on
  error, so this is a genuine empty success).
- **`unified_search_native` is UNCHANGED by this PR** (diff stat: no edits to
  it / `vector_operations_service` / `legacy.rs` search). So the break is
  **downstream** — the PR's SST-engine changes broke the **native-search read
  path** that `unified_search_native` invokes, while the `VectorSearchRequest`
  path still works.

### Most likely culprit (fits the symptom exactly)
The PR's **persistent L2 cache** (`crates/foundation/proximadb-cache/src/persistent_l2.rs`)
serves empty/stale reads to the **native-search** path; the `VectorSearchRequest`
path likely does not route through it (or routes differently). I.e. the
native-read path goes through the new L2 and gets nothing back, while the
request-read path bypasses it and reads the live memtable. A coalescing /
compaction read-path change is the secondary suspect.

### Ruled out (I verified each)
- **Engine:** both fusion and test_large_k use `Some("sst")` — not an engine mismatch.
- **Search-path identity fail-close:** the search path has **no** `parse::<u64>()`
  / `get_collection_object_id` gate (only the flush plan did). Not an identity fail-close.
- **`create_graph` flush:** `create_graph` → `create_graph_collection`; it does
  **not** flush `code_vecs`. The vectors stay in `code_vecs`'s memtable.
- **Error swallowing:** `retry_vector_search` **bails** (propagates) on
  exhaustion — it does not swallow to empty. The search returns `Ok([])`.

### Where to look (the fix)
Trace `unified_search_native_with_tenant_context` → the SST engine's
**native-search** dispatch, and **diff it against the `VectorSearchRequest`
search dispatch**. The divergence is the bug — most likely the native path
reads through the new persistent L2 (empty/stale) while the request path
doesn't. Check the L2 wiring in the native read path
(`src/storage/engines/sst/segment_format.rs` native scan + the survivor/invariants
caches' integration with `persistent_l2.rs`).

## Also remaining (NOT a CI-gating failure, but real): restart-recovery

`tests/embedded_flush_recovery.rs` (`embedded_flush_persists_and_recovers`,
`sst_block_serialization_roundtrip`, `test_large_k_search_returns_correct_count`)
fails **after reopen**: `Post-flush k=1000: 1000` ✅ but `Post-reopen k=1000: 0`
❌ — **segments are orphaned after reopen**.

- My derive-handle fix made the **pre-reopen** path work (flush no longer
  errors; 1000 results). The **post-reopen** loss remains.
- Root cause: the segment **path is keyed by `coll.id` (UUID)**, which is
  **re-minted on each `create_collection`** → reopen creates a fresh UUID →
  written segments (under the old UUID's path) aren't found → 0 results.
- Fix (per ADR-0083 rev2): key the segment path by the **stable composite**
  (`CollectionIdentity`, on `StorageAssignment`), not the ephemeral UUID.
  (CI may not currently gate on `embedded_flush_recovery` — confirm; it wasn't in
  the original #1325 failure set, only storage + graph were.)

## Identity context (read these first)

- **ADR-0083 rev2** (PR #1329, in flight → develop): the composite
  `CollectionIdentity {account, namespace, collection}` is the **sole canonical
  identity**; the global `object_id` u64 is **retired as an identity** (kept
  only as an optional ordering sequence). Supersedes ADR-031's u64-as-identity.
- **ADR-0083 D2:** admission/WAL/compaction key on the composite (or a
  **derived non-stored hash** for hot-path convenience — that's what my
  `stable_collection_handle` is).
- **The unstable-identity issue (`coll.id` = UUID) is the common thread** behind
  the restart-recovery orphaning AND a contributor to the search-resolution
  fragility. The full rev2 stable-composite migration resolves the restart-recovery.

## Recommended next steps (Codex)

1. **Fusion (the CI blocker):** trace `unified_search_native` → SST
   native-search; diff against `VectorSearchRequest`'s SST path; the divergence
   (likely persistent-L2 on the native-read path) is the bug. This is your SST/L2
   code — you'll recognize the native-vs-request read divergence fastest.
2. **Restart-recovery:** key the segment path by the stable composite (rev2),
   not the UUID. Confirm whether `embedded_flush_recovery` is CI-gated.
3. **Decide on my derive-handle** (`stable_collection_handle`): keep as the
   rev2-D2 derived handle, or rework to composite-keyed per the full migration.
4. **Rework the storage-test numeric mock** to composite-keyed once rev2 lands.

## Commits on this branch (mine)

- `8fcc57276` — fix(tests+storage): storage-test catalog-id mock + flush-plan
  derived-handle (rev2 D2).

My partial fixes are real improvements (storage green; flush-plan unblocked;
compiles clean) — build on them or rework per rev2. The valuable part of this
handoff is the **localization** (native-search SST path empty; request path
works; suspect persistent L2) — that should save you the debugging.
