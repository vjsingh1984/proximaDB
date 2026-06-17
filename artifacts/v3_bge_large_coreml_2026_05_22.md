# bge-large CPU vs CoreML — CoreML Still Loses + Breaks at Scale

Tested `BAAI/bge-large-en-v1.5` (1024-dim, ~440 MB ONNX, 24 layers,
335M params) through the v3 documents endpoint with both CPU and
CoreML execution providers. Hypothesis: the larger model amortizes
ANE dispatch overhead better than bge-small did.

**Result: hypothesis falsified again, and worse.** On bge-large
CoreML is 2-3× slower than CPU at small batches and **fails outright**
at 500-doc batches. CPU EP remains the correct default.

## Environment

- 10-core macOS arm64 (M-series Apple Silicon)
- `proximadb-server --features coreml`, version with
  `PROXIMADB_EMBED_VARIANT=large` env support
- Model staged at `/tmp/proximadb-models/staged-large/`
- 3 warmup calls per provider before timed runs

## Results — bge-large (ops/s, median over 3 runs)

| Path | CPU 50 | CPU 200 | CPU 500 | CoreML 50 | CoreML 200 | CoreML 500 |
|---|---:|---:|---:|---:|---:|---:|
| `sync_one_per_call` | 16.4 | 18.4 | 18.2 | 6.2 | 6.1 | **5.8** |
| `sync_one_big_call` | 23.1 | 27.1 | 25.2 | 10.2 | 7.6 | **FAIL** |
| `async_one_per_call_conc8` | 17.3 | 19.7 | 20.0 | 6.4 | 5.9 | **FAIL** |
| `async_chunked_conc8` | 23.2 | 24.8 | 24.2 | 12.4 | 13.9 | **FAIL** |

**FAIL** = HTTP 408 (request timeout, 60s budget exhausted) or HTTP 500
(server-side error during CoreML inference). One sync_one_per_call run
at 500 docs completed at 5.8 ops/s — every other 500-doc run errored.

## Comparison: bge-small vs bge-large on the same hardware

Apples-to-apples at 200 docs:

| Path | bge-small CPU | bge-large CPU | bge-small CoreML | bge-large CoreML |
|---|---:|---:|---:|---:|
| `sync_one_per_call` | 78.9 | 18.4 (−77 %) | 21.5 | 6.1 (−72 %) |
| `sync_one_big_call` | 335.8 | 27.1 (−92 %) | 82.5 | 7.6 (−91 %) |
| `async_chunked_conc8` | 306.6 | 24.8 (−92 %) | 115.0 | 13.9 (−88 %) |
| `async_one_per_call_conc8` | 148.3 | 19.7 (−87 %) | 23.6 | 5.9 (−75 %) |

CPU absorbs the 4-12× model-size jump roughly proportionally
(bge-large is ~12× the FLOPs of bge-small per inference). CoreML
suffers the same proportional drop AND keeps its bge-small overhead,
so the gap widens further.

## Why CoreML loses harder on bge-large

1. **More CPU↔ANE transfer per inference.** bge-large output is
   `[batch, seq, 1024]` vs bge-small's `[batch, seq, 384]` — 2.67× the
   tensor every round trip.
2. **24 layers vs 12 layers.** CoreML dispatches each ONNX op as a
   compiled kernel. Twice as many ops means twice as many dispatch
   crossings.
3. **Dynamic shape compilation overhead.** ORT's CoreML EP doesn't
   set `static_input_shapes` by default. Each new batch size (200,
   500) triggers CoreML to JIT-compile a new specialized graph. On
   bge-large that compilation evidently times out / OOMs at 500 docs.
4. **No fp16 quantization.** The ONNX files we downloaded are fp32.
   ANE's throughput advantage comes from fp16 / int8 quantized
   weights; fp32 forces hybrid CPU+GPU execution that pays the worst
   of both worlds.

## CPU bge-large remains usable but slow

At ~25 ops/s sync batch throughput, bge-large CPU is ~12× slower than
bge-small CPU. For workloads that genuinely need 1024-dim multilingual
embeddings, this is the cost of correctness on the current hardware /
model artifact combination. Throughput options:

- **fp16 ONNX**: export the model with `--dtype float16` via optimum.
  Halves memory transfer, often 1.5-2× faster CPU inference on Apple
  Silicon (AMX has fp16 paths).
- **Larger pool size**: pool=4 will not help single-batch paths (one
  batch only uses one session) but will help concurrent fan-out — same
  pattern documented in `v3_intra_op_override_sweep_2026_05_22.md`.
- **GPU execution provider** other than CoreML: not available on macOS
  without writing a Metal/MPS EP from scratch. On Linux, CUDA on a
  small Nvidia GPU would crush this — bge-large on a T4 hits ~500 ops/s
  in the OpenAI Whisper community benchmarks.

## When CoreML might still win

Not at these batch sizes with fp32 weights. To get a real ANE win
operators would need:

1. Convert the model to CoreML's `.mlmodel` / `.mlpackage` format
   directly (not ONNX→CoreML EP), using `coremltools` with the
   `ALL_COMPUTE_UNITS` target and fp16 weights.
2. Pin sequence length to 512 (the BGE max) and batch dimensions to
   fixed buckets (1, 8, 32) so CoreML can cache compiled graphs.
3. Use `ort::ep::CoreML::default().with_static_input_shapes(true)` —
   not exposed via our env-var override yet; would be a follow-up.

The current `coreml` feature is therefore **opt-in for experimentation
only** — the default `cpu` provider remains the production
recommendation for both bge-small and bge-large.

## Recommendations updated

| Workload | Recommended provider | Why |
|---|---|---|
| bge-small on macOS | **CPU** | 78-336 ops/s sync; 4-6× faster than CoreML |
| bge-large on macOS | **CPU** | 18-27 ops/s sync; CoreML 2-3× slower + fails at scale |
| bge-small on Linux Intel | CPU (consider `--features onednn`) | Intel-tuned kernels |
| bge-small on Linux NVIDIA | `--features cuda` | T4/A10 should beat embedded CPU by ~5-10× |
| bge-large anywhere on CPU | Accept the throughput hit OR use fp16 export OR move to GPU | bge-large is 12× FLOPs of bge-small |

## Artifacts

- `artifacts/v3_large_cpu_{50,200,500}_2026_05_22.json` — CPU bge-large
- `artifacts/v3_large_coreml_{50,200,500}_2026_05_22.json` — CoreML bge-large (500-doc partial)
- `artifacts/v3_coreml_ep_2026_05_22.md` — bge-small CoreML report (companion)
- This report
- New: `clients/python-embedded/benchmarks/server_v3_documents_embedding_large.py`
