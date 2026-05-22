# v3 Documents Endpoint with Real ONNX Embedding — 2026-05-22

End-to-end bench of `POST /api/v3/collections/{id}/documents` with the real
`BAAI/bge-small-en-v1.5` ONNX session loaded into the server. Replaces the
synthetic-fallback numbers in `artifacts/v3_embed_*_2026_05_22.json` with
production-realistic timings.

## What changed in the stack

| Change | Reason |
|---|---|
| Added `ort` 2.0.0-rc.10 + `ndarray` 0.17 to `proximadb-embedding/Cargo.toml` under feature `onnx`. | Was previously absent — the `onnx` feature was an empty flag. |
| Implemented `BgeModel::embed_batch_onnx` in `src/models/bge.rs`. | Was `unimplemented!()`. Now: tokenize → input_ids/attention_mask/token_type_ids → `session.run` → masked mean-pool → L2 normalize. |
| Wrapped `Session` in `std::sync::Mutex`. | `ort::Session::run` is `&mut self` in 2.0.0-rc.x. Single-session per variant; multi-session pool is a follow-up. |
| Made `ModelRegistry` lazy per-variant. | BGE-large + BGE-m3 don't need to load if no request resolves to them. |
| **Removed the synthetic fallback from production paths**. | Per user directive: tests-only via `bge::testing::synthetic_vector` (gated `#[cfg(test)]`). Production now returns `ModelUnavailable` with a clear message when the model file is missing instead of silently producing meaningless vectors. |
| Added `onnx` passthrough feature to `apps/proximadb-server/Cargo.toml`. | So the binary can opt into real inference at build time. |

Build: `cargo build --release -p proximadb-server --features onnx`.
Binary: 108 MB (was 80 MB without ORT). ORT runtime auto-downloaded by
`ort/download-binaries` at first build.

## Model staging

- ONNX model + tokenizer downloaded from HF (`BAAI/bge-small-en-v1.5`,
  `onnx/model.onnx` + `tokenizer.json`).
- Staged at `/tmp/proximadb-models/staged/bge-small-en-v1.5.onnx` +
  `tokenizer.json`.
- Server resolves paths via `PROXIMADB_EMBED_MODEL_DIR` and
  `PROXIMADB_TOKENIZER_PATH`. First request to a BGE route triggers the
  ONNX session load (lazy, ~165 ms cold-start).

## Bench results — real ONNX inference

Scale: 50 / 200 / 500 docs per run × 3 runs. Median ops/s.

| Path | 50 docs | 200 docs | 500 docs |
|---|---:|---:|---:|
| `flight.text_only.sync` (Arrow Flight DoPut, server embeds) | **304.6** | **340.4** | **326.5** |
| `v3.embed.sync_one_big_call` (one POST with all docs) | 269.0 | 335.8 | **341.5** |
| `v3.embed.async_chunked_conc8` (8 chunks via `aiohttp.gather`) | 247.6 | 306.6 | **342.2** |
| `v3.embed.async_one_per_call_conc8` (1 doc/POST, 8 concurrent) | 144.8 | 148.3 | 145.2 |
| `v3.embed.sync_one_per_call` (1 doc/POST, sequential) | 77.2 | 78.9 | 76.5 |

## Synthetic vs real — the gap

For the same 500-doc bench, comparing to the synthetic-fallback numbers
collected before the ONNX implementation landed
(`artifacts/v3_embed_500_2026_05_22.json`):

| Path | Synthetic | Real ONNX | Slowdown |
|---|---:|---:|---:|
| `sync_one_per_call` | 149.6 / s | 76.5 / s | **2.0×** |
| `sync_one_big_call` | 58 308 / s | 341.5 / s | **171×** |
| `async_one_per_call_conc8` | 1 191 / s | 145.2 / s | **8.2×** |
| `async_chunked_conc8` | 54 719 / s | 342.2 / s | **160×** |
| `flight.text_only.sync` | 139 098 / s | 326.5 / s | **426×** |

This is the gap I called out before the implementation: the synthetic
fallback was returning hash-derived 384-dim vectors with no real compute,
so single-doc paths (which were RTT-dominated) only slowed 2×, while
batch paths (which were genuinely free in synthetic) slowed by 160-426×.

**Real-ONNX numbers are now production-credible.**

## "Async dominates sync when embedding is on the critical path" — validated

The user's hypothesis from earlier in this session:

| Path | sync ops/s | async ops/s | Async/Sync |
|---|---:|---:|---:|
| `sync_one_per_call` vs `async_one_per_call_conc8` @ 50 | 77 | 145 | **1.88×** |
| @ 200 | 79 | 148 | **1.87×** |
| @ 500 | 77 | 145 | **1.90×** |

Async **does** beat sync by ~1.9×, but **not** by 8× as a naive concurrency
calc would predict. The cap is the server-side `Mutex<Session>` — only one
inference at a time. Async lets multiple HTTP requests sit at the lock
queue, but the underlying ONNX work is serialized.

For higher throughput, the server needs a **session pool** (e.g.,
`PROXIMADB_EMBED_SESSIONS=N`, round-robin across N sessions of the same
variant). The `ort` runtime supports multiple sessions sharing the same
underlying model weights via memory mapping, so the RAM cost is only the
delta of per-session state. This is a clean follow-up to the present PR.

## "Big batch ≈ chunked async" — model is the bottleneck

At 500 docs:

- `sync_one_big_call = 341.5/s` — one HTTP POST, server embeds all 500 in one batch.
- `async_chunked_conc8 = 342.2/s` — 8 HTTP POSTs of ~62 docs each, concurrent.

These converge because both flows hit the same single `Session` and the
Mutex serializes them. The work-per-doc on the model is ~3 ms (CPU
inference for 64-token sequences); model-bound throughput caps at
~340 docs/sec regardless of how requests arrive.

## Arrow Flight observation

`flight.text_only.sync = 326.5/s` is in the same range as the REST batch
path. Both go through the same `embed_text_only_records` in
`network/arrow_ipc/service.rs`, then the same Mutex-guarded session. The
Arrow Flight transport advantage that dominated when we benched
pre-computed vectors (255 k/s vs 31 k/s for REST) is invisible here
because the wire-protocol cost is amortized over the much larger ONNX
inference cost.

**Takeaway**: for embedding-bound workloads, the wire protocol matters
~10× less than the model throughput. For pre-computed-vector workloads,
Arrow Flight is 7-8× ahead. Choose the surface that matches your
workload's bottleneck.

## Open follow-ups

1. **Session pool** (`PROXIMADB_EMBED_SESSIONS=N` env, round-robin in
   `ModelRegistry::bge`). Unlocks parallel inference. Expected: ~N×
   throughput up to CPU core count, then flattens.
2. **GPU sessions** via `ort` execution providers (CoreML on macOS, CUDA
   on Linux). Order of magnitude jump on BGE-small.
3. **Model-distribution mechanics** (separate doc): pre-converted ONNX
   files in an ADLS/S3 bucket, init-container fetch into emptyDir mounted
   at `/var/lib/proximadb/models/`. See `cross_surface_writes_2026_05_22.md`
   appendix for the Terraform sketch.
4. **`PROXIMADB_TOKENIZER_PATH` consolidation**: today the chunker's
   `SharedTokenizer` and the BGE inference path both read this env var
   separately; should be unified into one initialization.

## Artifacts

- `artifacts/v3_real_50_2026_05_22.json` — 50-doc real ONNX
- `artifacts/v3_real_200_2026_05_22.json` — 200-doc real ONNX
- `artifacts/v3_real_500_2026_05_22.json` — 500-doc real ONNX
- `artifacts/v3_embed_*_2026_05_22.json` — synthetic-fallback baseline (kept for comparison)
- This report
