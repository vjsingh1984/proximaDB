# Intra-Op Thread Tuning — Negative Result, Default Reverted

Followup to `artifacts/v3_pool_sweep_2026_05_22.md`. Hypothesis tested:
"setting `intra_op_threads = max(1, cores / pool_size)` per session
will fix the batch-throughput regression observed at pool>2."

**Result: hypothesis falsified.** Tuning made the regression worse for
batch workloads. The default is therefore reverted to leave
`intra_op_threads` unset — letting ONNX Runtime pick its own (all-cores)
default. The env var `PROXIMADB_EMBED_INTRA_OP_THREADS` is preserved as
an opt-in for niche workloads.

## Numbers (10-core macOS arm64, 200 docs × 3 runs, real BGE-small ONNX)

### Default ORT threading (untuned, original sweep)

| Path | pool=1 | pool=2 | pool=4 | pool=8 |
|---|---:|---:|---:|---:|
| `v3.embed.sync_one_per_call` | 77.0 | 76.4 | 76.3 | 76.1 |
| `v3.embed.sync_one_big_call` | 330.8 | 330.3 | 297.6 | 298.3 |
| `flight.text_only.sync` | 323.1 | 336.1 | 299.3 | 301.4 |
| `v3.embed.async_chunked_conc8` | 281.6 | **375.9** | 347.2 | 276.5 |
| `v3.embed.async_one_per_call_conc8` | 144.3 | 234.0 | 259.0 | **310.0** |

### Force `intra_op = cores/pool` (tuned, this run)

intra_op resolved to: pool=1→10, pool=2→5, pool=4→2, pool=8→1.

| Path | pool=1 | pool=2 | pool=4 | pool=8 |
|---|---:|---:|---:|---:|
| `v3.embed.sync_one_per_call` | 65.9 | 66.7 | 73.8 | 60.6 |
| `v3.embed.sync_one_big_call` | 251.6 | 232.9 | **142.3** | **71.3** |
| `flight.text_only.sync` | 241.0 | 239.3 | 134.9 | 72.3 |
| `v3.embed.async_chunked_conc8` | 178.5 | 231.1 | 300.6 | 252.1 |
| `v3.embed.async_one_per_call_conc8` | 92.2 | 177.4 | 256.8 | 267.6 |

### Why the tuning hurt

For a "single batch of 200 docs" path, exactly **one session** in the
pool handles the request — the rest sit idle. Constraining that session
to `cores/pool` intra-op threads doesn't unlock parallelism elsewhere;
it just slows the one session that's actually doing work.

- pool=4, intra_op=2: the active session uses 2 of 10 cores. The
  ONE inference takes ~2.3× longer than the unconstrained version.
- pool=8, intra_op=1: single-threaded matmul. The ONE inference is
  ~4-5× slower.

The "oversubscription" cost the original sweep saw (-10% at pool=4)
is much smaller than the "under-parallelism" cost of forcing
single-threaded matmul on the actually-busy session.

### Even the concurrent path didn't win

`async_one_per_call_conc8` at pool=8 went from 310 ops/s (untuned) to
267 ops/s (tuned). With 8 concurrent inferences each pinned to 1
thread, the inner matmul kernels can't parallelize, and 8×slow
sequential matmuls > 1×fast parallel matmul on this BGE-small workload
where each inference is already small.

## Code outcome

- `BgeModel::initialize` calls `with_intra_threads(N)` **only** when
  `PROXIMADB_EMBED_INTRA_OP_THREADS` is explicitly set to a valid
  positive integer. Default: leave unset → ORT picks (typically all
  cores).
- Helper renamed from `intra_op_default` → `intra_op_suggested` to
  emphasize it's a guideline, not the default.
- `resolve_intra_op_threads` now returns `Option<usize>` instead of
  `usize`. `None` = "use ORT default"; `Some(N)` = explicit override.
- 16 unit tests pass (15 from the previous TDD pass, refactored + 1
  new test for whitespace trimming on the override env var).

## When the env var IS useful

Override `PROXIMADB_EMBED_INTRA_OP_THREADS=N` when:
- You're running a **fan-out workload** with N>>cores concurrent small
  inferences (e.g. SaaS request-per-doc traffic at high QPS).
- Profiling shows ORT thread thrash in `perf` / `Instruments`.
- You have multiple tenants on shared compute and need to bound per-
  session CPU.

Default behavior (leave unset) wins for everyone else.

## Recommendations updated

| Workload | Pool | intra_op env |
|---|---|---|
| Default mixed | 2 | unset (let ort decide) |
| Batch-heavy (large POSTs) | 1 | unset |
| Fan-out (many concurrent small calls) | 4-8 | maybe set to 1-2 if thrash is observed |
| Profiling/research | n/a | tune freely |

## Artifacts

- `artifacts/v3_real_pool{1,2,4,8}_200_2026_05_22.json` — untuned (the good numbers)
- `artifacts/v3_real_pool{1,2,4,8}_tuned_200_2026_05_22.json` — tuned (regressed)
- This report
