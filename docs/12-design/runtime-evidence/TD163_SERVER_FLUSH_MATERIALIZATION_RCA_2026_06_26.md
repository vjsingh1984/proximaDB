# TD-163 / TD-165 — Server Flush Materialization & the Cold-Read Recall Block (RCA, 2026-06-26)

Runtime RCA from attempting TD-163 (make the server materialize vector data to a
storage segment on flush, so the A6 storage-write fence becomes load-bearing on the
live path). **Outcome: TD-163 is blocked on a pre-existing cold-read recall defect
(TD-165) and was parked rather than shipped — materializing today regresses
recall-after-restart.**

## What TD-163 changed (parked on `feat/td163-server-flush-materialization`)
- A shared `storage::flush_materializer::materialize_collection` helper (catalog →
  `StorageFormatFactory::create_from_proto_async` → `engine.flush(collection_config)`
  + WAL cleanup + the A6 fence), replacing the server's dead `sst_storages`/coordinator
  registry path. `StorageEngine::flush_memtable_to_storage` resolves metadata from
  `list_collections_from_catalog()` and calls it; `EmbeddedDb::flush` calls the same
  helper (dedup).
- **Verified working:** server insert (500) → SIGINT → `Flushed 1 collections, 500
  vectors`; an `L0_*.sst` segment + AXIS `ivf.bin` appear on disk (previously the
  server materialized nothing — WAL only).

## The blocker (runtime-pinned)
With the WAL freed after a successful flush, restart-recall routes through the cold
SST read path, which returns the **wrong** nearest neighbor. Reproduction: insert 2000
vectors `vec_i[j] = i*0.01 + j*0.001`, flush (SIGINT), restart, query `vec_0`'s vector:

```
PRE-flush  (FP32 memtable, brute-force): top5 = vec_0, vec_1, vec_2, vec_3, vec_4   ✅ exact
POST-restart (cold SST read):            top5 = vec_8, vec_9, vec_18, vec_19, vec_20 ❌ vec_0..7 lost
```

Debug trace of the three-stage cold search:
```
Stage 1 (WAL/memtable):  No unflushed batches → 0 results   (WAL freed, memtable empty)
Stage 2 (AXIS IVF):      Deserialized IVF index with 0 vectors → 0 results
Stage 3 (SST scan):      read_all_for_compaction: 3 blocks, 895+897+208 = 2000 records read
                         → still returns vec_8,9,18,19,20 (wrong)
```

Two distinct defects in the cold path (TD-165):
1. **Persisted IVF index is empty.** AXIS index build is asynchronous ("EventLog
   consumer builds AXIS in the background" after flush); at shutdown the SST is written
   but the index build hasn't populated/persisted, so `ivf.bin` saves with **0 vectors**.
2. **The FP32 cold scan misranks.** Stage 3 reads *all* 2000 records (the SST is ~583 KB
   ≈ 2000×64×4 B, i.e. FP32 is stored), yet returns the wrong top-k — a genuine
   distance/ranking bug in the cold SST search (not an ANN-approximation artifact and not
   a "blocks missing" bug).

## Why a shortcut doesn't work
"Keep the WAL so recovery replays it (FP32) for exact recall" was tried and **fails**:
a successful `engine.flush()` marks the batches **flushed in the manifest**
(`manifest/service.rs` — `Marked N entries as Flushed`), independent of WAL-file
deletion. Recovery (`ViaMemtable`) then *skips* flushed batches, so the memtable stays
empty and the cold path is still the only recall source. Net: keeping the WAL files does
not restore recall.

## Dependency inversion
TD-165 (cold-read recall) is therefore a **prerequisite** for TD-163, not a follow-up.
Until the cold path returns exact NN (populate + persist the IVF index at flush *or*
rebuild it on recovery; and fix the FP32 cold-scan ranking), the server must not free the
WAL on flush. The pre-existing `tests/embedded_flush_recovery.rs::sst_block_serialization_roundtrip`
failure is the same defect (embedded already frees the WAL) — it is the natural ratchet
test for TD-165.

## Status
- A6 fence enforcement (#360): merged, unaffected.
- TD-163 (server materialization): **parked** — code on `feat/td163-server-flush-materialization`,
  not merged (would regress recall). Reopen once TD-165 lands.
- TD-165 (cold-read recall): filed as the prerequisite.
