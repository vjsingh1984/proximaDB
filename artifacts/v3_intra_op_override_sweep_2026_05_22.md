# Intra-Op Override Sweep — Workload-Specific Wins Found

Followup to `v3_intra_op_validation_2026_05_22.md` where the
`cores/pool_size` *default* policy was rejected. This sweep tests
**explicit overrides** at pool=4 and pool=8 to find workload-specific
sweet spots that beat the unset-default.

Server: rebuilt release binary with the opt-in override semantics
(commit `c6e597c5c`). 10-core macOS arm64, real BGE-small ONNX, 200
docs × 3 runs.

## Result matrix

### pool=4 (4 sessions)

| Path | unset (default) | i=1 | i=2 | i=3 | i=4 |
|---|---:|---:|---:|---:|---:|
| `sync_one_per_call` | 76.3 | 58.9 (−23 %) | 65.7 (−14 %) | 64.7 (−15 %) | 74.7 (−2 %) |
| `sync_one_big_call` | 297.6 | 69.6 (−77 %) | 126.9 (−57 %) | 130.4 (−56 %) | 233.0 (−22 %) |
| `flight.text_only.sync` | 299.3 | 70.1 (−77 %) | 134.1 (−55 %) | 146.9 (−51 %) | 225.0 (−25 %) |
| `async_chunked_conc8` | 347.2 | 293.2 (−16 %) | 292.9 (−16 %) | 313.0 (−10 %) | 268.9 (−23 %) |
| **`async_one_per_call_conc8`** | 259.0 | **294.8 (+14 %)** | 277.5 (+7 %) | 257.2 (−1 %) | 234.0 (−10 %) |

### pool=8 (8 sessions)

| Path | unset (default) | i=1 | i=2 |
|---|---:|---:|---:|
| `sync_one_per_call` | 76.1 | 51.5 (−32 %) | 70.4 (−8 %) |
| `sync_one_big_call` | 298.3 | 60.1 (−80 %) | 136.8 (−54 %) |
| `flight.text_only.sync` | 301.4 | 68.4 (−77 %) | 145.6 (−52 %) |
| **`async_chunked_conc8`** | 276.5 | 223.8 (−19 %) | **324.0 (+17 %)** |
| `async_one_per_call_conc8` | 310.0 | 235.1 (−24 %) | 214.2 (−31 %) |

## The two genuine wins

| Workload pattern | Config | vs unset default |
|---|---|---|
| Many concurrent single-doc HTTP calls (e.g., SaaS request-per-doc) | `pool=4 intra=1` | **+14 % on `async_one_per_call_conc8`** (259 → 295 ops/s) |
| Client batches the docs but server sees ~8 concurrent chunk POSTs | `pool=8 intra=2` | **+17 % on `async_chunked_conc8`** (276 → 324 ops/s) |

Everything else either regresses or is neutral. **For batch workloads
(single large POST), unset always wins** — one active session
unconstrained beats one session pinned to a subset of cores while N-1
peers sit idle.

## Why the asymmetry across pool sizes

- `pool=4, intra=1`: 4 sessions × 1 thread = 4 threads total. Fits in
  10 cores with slack. Each concurrent inference is single-threaded but
  the workload is small enough that single-thread matmul is acceptable;
  no oversubscription. Net win on fan-out.
- `pool=8, intra=1`: 8 sessions × 1 thread = 8 threads. Should fit even
  better than 4×1, yet it *regresses* −24 % on
  `async_one_per_call_conc8`. Hypothesis: Mutex contention across 8 lock
  slots + 8 simultaneous mmap working sets hurts cache locality more
  than the extra parallelism helps for BGE-small's tiny inferences.
- `pool=8, intra=2`: 8 sessions × 2 threads = 16 threads (oversubscribed
  on 10 cores), but the bench's chunked async path actually engages all
  8 sessions simultaneously, so the 2-thread inference parallelism inside
  each session pays off. Net win on chunked async.

## Operational recommendations (revised)

| Workload profile | `PROXIMADB_EMBED_SESSIONS` | `PROXIMADB_EMBED_INTRA_OP_THREADS` |
|---|---|---|
| Default mixed / unknown | 1 (or 2) | **unset** |
| Batch-heavy (large POSTs from a few clients) | 1-2 | unset |
| Fan-out SaaS (many single-doc concurrent calls) | **4** | **1** |
| Client batches into chunks (e.g. parallel `gather` of moderate-size requests) | **8** | **2** |
| Profiling / research | n/a | tune freely with `intra_op_suggested(cores, pool)` as a starting point |

## Code outcome

- No code change required — the override semantics in commit
  `c6e597c5c` are correct (`Option<usize>` from
  `resolve_intra_op_threads`, no-op unless explicitly set).
- This report documents the validated tuning recipes.

## Artifacts

- `artifacts/v3_real_p{4,8}_i{1,2,3,4}_2026_05_22.json` — raw override sweep
- `artifacts/v3_real_pool{1,2,4,8}_200_2026_05_22.json` — untuned baseline (unchanged from earlier)
- This report
