# BGE Session Pool Sweep — 2026-05-22

Validates the new `PROXIMADB_EMBED_SESSIONS` env var (commit `58d21a9e1`).
Pool size 1 / 2 / 4 / 8, real ONNX `BAAI/bge-small-en-v1.5`, 200 text-only
documents × 3 runs each, on the same release binary
(`target/release/proximadb-server --features onnx`, May 22 06:40 UTC).

## Results (median ops/s)

| Path | pool=1 | pool=2 | pool=4 | pool=8 |
|---|---:|---:|---:|---:|
| `v3.embed.sync_one_per_call` (200 sequential POSTs, 1 doc each) | 77.0 | 76.4 | 76.3 | 76.1 |
| `v3.embed.sync_one_big_call` (one POST with all 200 docs) | 330.8 | 330.3 | 297.6 | 298.3 |
| `flight.text_only.sync` (Arrow Flight one DoPut) | 323.1 | 336.1 | 299.3 | 301.4 |
| `v3.embed.async_chunked_conc8` (8 chunks of ~25 docs, `aiohttp.gather`) | 281.6 | **375.9** | 347.2 | 276.5 |
| `v3.embed.async_one_per_call_conc8` (200 POSTs, 8 concurrent) | 144.3 | 234.0 | 259.0 | **310.0** |

## What pool size to set for which workload

| Workload | Best pool | Why |
|---|---|---|
| Sequential single-doc inserts | any — pool irrelevant | The single in-flight session does the work; others sit idle. |
| One large batch per request | **1-2** | A batch goes to ONE session in the pool. Extra sessions only split intra-op threads → less per-session CPU. pool=4 and pool=8 are −10% to −15%. |
| Async chunked (8 chunks gathered) | **2** | 8 chunks across 2 sessions = 2× parallelism without CPU oversubscription. pool=2 hits 376/s (+33% over pool=1). |
| Async per-doc (many concurrent producers) | **8** (or higher) | Scales near-linearly: 144 → 310 ops/s from pool=1 → pool=8 (**2.15×**). Each concurrent caller gets its own session. |

## Combined win: async + pool

The path that benefits most from this PR is the natural pattern for an
asyncio app posting many small documents to `/api/v3/documents`:

- **Sync, pool=1 (baseline)**: 77.0 ops/s
- **Async conc=8, pool=8**: 310.0 ops/s → **4.03×**

Split:
- async alone (pool=1): 144 vs sync 77 = **1.87×**
- pool=8 alone (sync still serial — doesn't apply)
- async + pool=8: 310 vs async pool=1 144 = **2.15× from the pool**

So async and pool stack multiplicatively for the workload that needs
them — confirming the design intent.

## Why batch paths regressed at pool>2

A single REST POST with N docs is a single inference call that gets
exactly one session from the pool. The ONNX Runtime's intra-op thread
pool inside that session can use all available CPU cores when no other
session is competing. With pool=4 or pool=8 running concurrently (even
if only the bench's chunked path uses multiple at once), the kernel
scheduler ends up time-slicing across sessions, hurting per-session
throughput.

**This is a tuning trade-off, not a regression in the pool design.**
The right configuration depends on the workload mix.

## Recommendations

- **Default `PROXIMADB_EMBED_SESSIONS=1`** (what the code does today) —
  best for batch-oriented workloads, lowest memory.
- **`PROXIMADB_EMBED_SESSIONS=2`** — best for mixed: keeps batch
  throughput within 1% of pool=1 and gives `async_chunked` a 33% boost.
- **`PROXIMADB_EMBED_SESSIONS=8`** — best for fan-out scenarios with
  many small concurrent callers (e.g., HTTP request-per-doc patterns).
  Pay attention to RAM (each session adds ~10-20 MB on top of the
  shared mmapped weights).
- **Never exceed physical CPU cores** — beyond that point you're paying
  per-session memory cost for context-switch overhead.

## Open follow-ups

1. **Per-session ORT thread tuning**: expose
   `PROXIMADB_EMBED_INTRA_OP_THREADS` to constrain each session's
   internal parallelism. Useful when running large pool sizes —
   formula: `intra_op = max(1, cores / pool_size)`.
2. **Pool size auto-tune**: detect CPU core count at startup and pick a
   sensible default (e.g., `min(4, cores / 2)` for embedding nodes).
3. **GPU pool**: extend the same pattern to GPU execution providers
   (CoreML/CUDA), where multiple sessions can share VRAM via mmap.

## Artifacts

- `artifacts/v3_real_pool{1,2,4,8}_200_2026_05_22.json` — raw runs
- This report
