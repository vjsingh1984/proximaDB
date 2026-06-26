# TD-163 — Server Vector-Flush Materialization: Resolution Evidence (2026-06-26)

**Status: Resolved.** The server's only live `do_flush` boundary (SIGINT graceful
shutdown) now materializes vector data to an SST segment **and frees the WAL**, so the
segment — not a WAL replay — is the durable restart-recall source. The A6 storage-write
fence is load-bearing on that path (default-OFF).

This complements `TD163_SERVER_FLUSH_MATERIALIZATION_RCA_2026_06_26.md` (the root-cause +
"keep-WAL does not help" finding) and `TD165_*` (the cold-read recall fix that was the true
prerequisite).

## What changed (the completion)

`StorageEngine::flush_memtable_to_storage` (the SIGINT path, `src/storage/engine.rs`)
already resolved each collection's engine from the catalog and called the shared
`storage::flush_materializer::materialize_collection` (landed with #369) — but it passed
`free_wal = false`, keeping the WAL so restart recall replayed it into the FP32 memtable.
That made the just-written SST redundant for restart (recovery replayed the WAL and ignored
the segment), and it was only done because the cold SST read path was broken (TD-165).

TD-165 is Resolved (#369): IVF posting lists are populated before persist and the SST route
honors `SearchMode`. With cold-read recall fixed, this change flips the server to
**`free_wal = true`** (matching embedded): the WAL is cleared + deleted after the segment is
written, so the materialized SST is the durable restart-recall source. Keep-WAL never helped
recall anyway — `engine.flush()` carries the `batch_ids`, so recovery treats them as flushed.

`free_wal` is retained on `materialize_collection` as an explicit escape hatch (`false` ⇒
keep the WAL so recovery replays it, bypassing the cold path) but **no caller uses it today**.

## Runtime trace (server binary, `apps/proximadb-server`, build off develop @ e1fe54c00)

Create collection (sst, dim 8) → insert 500 vectors → `kill -INT`:

```
🛑 STORAGE_ENGINE: Found 1 collections with unflushed data: ["b6f41320-…"]
✅ STORAGE_ENGINE: Flushed collection 'b6f41320-…': 500 vectors, 32727 bytes
🛑 STORAGE_ENGINE: Flush complete — 1 collections, 500 vectors, 32727 bytes
```

On-disk after shutdown — **the SST segment is materialized; the vector WAL is freed**:

```
<root>/d1/b6f41320-…/data/L0_20260626T190016_0ec09500.sst   ← materialized segment
<root>/d1/b6f41320-…/data/__model/pca_model.bin
<root>/data/axis_indexes/b6f41320-…/ivf.bin                  ← AXIS index built
(no vector *.wal — freed by free_wal=true)
```

Restart recovery correctly finds nothing to replay from the freed WAL and serves recall
from the segment.

## Recall gate (cold SST read is no worse than hot memtable)

`tests/embedded_flush_recovery.rs::cold_read_recall_survives_flush_and_reopen` — a
**strong** cold-read recall ratchet (the gate TD-165 *should* have had). The pre-existing
`sst_block_serialization_roundtrip` asserted recall only on `vec_0` — a trivial,
well-separated corner case whose top-1 stays correct even when the cold path misranks every
other query; that weak assertion is why a cold-read regression slipped through and TD-165 was
marked Resolved prematurely.

The new gate uses well-separated, deterministic, seeded-random **unit** vectors (each
inserted vector is its own unambiguous exact NN), measures recall@10 on the **hot** path
(memtable, pre-reopen) and the **cold** path (post-reopen, WAL freed → recall from the SST),
and asserts cold does not regress hot. Measured on develop:

```
hot  (memtable)        recall@10 = 1.000
cold (SST, post-reopen) recall@10 = 1.000
```

Both paths achieve perfect recall@10 — the cold SST read path is sound, and `free_wal=true`
introduces no regression. Wired into the `qa-gate.yml` recall tier (develop→qa).

## Known residual (minor, not a correctness bug)

The server shutdown-flush path does **not** call `update_stats` (embedded does, via
`shared_services`), so the catalog `record_count` reads 0 after a server-side flush+restart.
Search is unaffected (it reads the SST directly, proven above); only the optimizer's
row-count hint is stale. Follow-up: refresh catalog stats from the materialized segment on
the server flush path.
