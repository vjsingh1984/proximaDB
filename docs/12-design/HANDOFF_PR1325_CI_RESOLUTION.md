# PR #1325 CI diagnosis and resolution

**Date:** 2026-07-30
**Status:** Storage and flush-plan failures are fixed in `8fcc57276`. The
graph/fusion failure was reproduced locally and fixed by preserving the
collection storage lookup key across the tenant-aware native-search boundary.

## What #1325 is

`feat(storage): reduce GETs with persistent cache and admitted flushes` — the
get-reduction stack: persistent local-disk L2 for PAX invariant + survivor
caches (D1–D6), admitted flushes with typed WAL backpressure, higher-level
compaction consolidation, PAX probe-range coalescing, stable-id migration, and
a hardened SIFT bench harness. +6.5k lines, 65 files.

## Initial CI state

Two CI jobs were failing: **Rust Integration Tests (storage)** and **Rust
Integration Tests (graph)**. The storage failure and flush-plan regression were
fixed first; the graph/fusion failure was then isolated and resolved as
described below.

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

## Graph/fusion CI failure — root cause and fix

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

### Root cause

`unified_search_native_with_tenant_context` validated the collection and then
converted the returned numeric `CollectionObjectId` handle to a string before
calling `unified_search_native`. That handle is valid for in-memory catalog
indexing and admission, but it is not the collection key used by the
WAL/memtable/SST read path. Native search therefore selected a different,
empty storage namespace and returned `Ok([])`.

The fix preserves the storage lookup key: a tenant-scoped call validates access
and delegates with the tenant-resolved collection id; a single-tenant call
delegates with the caller's collection key.

### Ruled out (I verified each)
- **Engine:** both fusion and test_large_k use `Some("sst")` — not an engine mismatch.
- **`create_graph` flush:** `create_graph` → `create_graph_collection`; it does
  **not** flush `code_vecs`. The vectors stay in `code_vecs`'s memtable.
- **Error swallowing:** `retry_vector_search` **bails** (propagates) on
  exhaustion — it does not swallow to empty. The search returns `Ok([])`.
- **Persistent L2:** the failing vectors were never flushed, so no PAX segment
  or L2 cache lookup could participate in this result.

### Regression evidence

Before the fix:

- `embedded_code_graph_parity_e2e`: 2 passed, 2 failed.

After preserving the storage lookup key:

- `embedded_code_graph_parity_e2e`: 4 passed, 0 failed.
- `sst_compaction_execution_test`: 2 passed, 0 failed.
- `sst_ivf2_compaction_route_proof_test`: 2 passed, 0 failed.
- `cargo clippy -p proximadb --lib -- -D warnings`: passed.

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

## Remaining work

1. **Rerun CI:** confirm graph and storage integration jobs on the fix commit.
2. **Restart recovery:** key the segment path by the stable composite (rev2),
   not the UUID. Confirm whether `embedded_flush_recovery` is CI-gated.
3. **Decide on the derived handle** (`stable_collection_handle`): keep as the
   rev2-D2 derived handle, or rework to composite-keyed per the full migration.
4. **Rework the storage-test numeric mock** to composite-keyed once rev2 lands.

## Commits on this branch (mine)

- `8fcc57276` — fix(tests+storage): storage-test catalog-id mock + flush-plan
  derived-handle (rev2 D2).

The storage fixes are retained as a bridge to ADR-0083 rev2. The graph failure
was an identity-domain mix-up at the search boundary, not a cache-data
correctness defect.
