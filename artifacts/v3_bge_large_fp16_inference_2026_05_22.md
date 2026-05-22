# bge-large fp16 Inference — Modest But Real Speedup

Followup to `v3_bge_large_coreml_2026_05_22.md` (where CoreML lost on
bge-large) and `v3_intra_op_validation_2026_05_22.md` (where the
default ORT threading proved optimal). Exported bge-large to fp16 and
re-benched.

## Hypothesis

fp16 weights + fp16 internal ops + AMX fp16 matmul on Apple Silicon
should give 1.5-2× CPU speedup on bge-large.

## Result

**Falsified at large batches, partially confirmed at small batches.**
fp16 wins +7% to +19% at 50 docs, drops to roughly flat at 500 docs.
The 2× win expected from "fp16 weights + AMX fp16" is not materializing
on this ort 2.0.0-rc.12 binary — either ORT isn't engaging AMX fp16
matmul, or the bottleneck is elsewhere (output conversion, allocator).

## Setup

- 10-core macOS arm64 (Apple Silicon M-series)
- `optimum-cli export onnx --library transformers --task feature-extraction --dtype fp16 BAAI/bge-large-en-v1.5`
- Server release built with `--features coreml`, run with
  `PROXIMADB_EMBED_VARIANT=large` + `PROXIMADB_EMBED_PROVIDER=cpu`
- Server-side Rust extract code updated to try `f32` first, fall back
  to `half::f16` and promote element-wise to `f32` (new `half` dep
  on the `onnx` feature)
- Bench: 50 / 200 / 500 docs × 3 runs each, `concurrency=8`

## Numbers (median ops/s)

| Path | fp32@50 | fp32@200 | fp32@500 | fp16@50 | fp16@200 | fp16@500 |
|---|---:|---:|---:|---:|---:|---:|
| `sync_one_per_call` | 16.4 | 18.4 | 18.2 | **18.2** | 18.6 | 17.7 |
| `sync_one_big_call` | 23.1 | 27.1 | 25.2 | **27.4** | **28.7** | 25.3 |
| `async_chunked_conc8` | 23.2 | 24.8 | 24.2 | **24.8** | **26.5** | 24.6 |
| `async_one_per_call_conc8` | 17.3 | 19.7 | 20.0 | **20.4** | 20.5 | 19.6 |

## Delta (fp16 / fp32, percentage)

| Path | 50 | 200 | 500 |
|---|---:|---:|---:|
| `sync_one_per_call` | **+11.1 %** | +1.1 % | −2.9 % |
| `sync_one_big_call` | **+18.9 %** | +6.0 % | 0.0 % |
| `async_chunked_conc8` | +7.1 % | +6.7 % | +1.7 % |
| `async_one_per_call_conc8` | **+17.8 %** | +4.2 % | −2.0 % |

## Why the win is smaller than expected

1. **AMX fp16 matmul may not be engaged.** ORT 2.0.0-rc.12's CPU EP
   uses Apple Accelerate framework when the dtype is fp32; for fp16
   it likely falls back to scalar / NEON paths without the
   matrix-coprocessor speedup we'd see from AMX-aware code.
2. **fp16 → fp32 promotion at the output boundary.** The Rust
   `embed_batch_onnx` extracts `half::f16` and promotes element-wise
   to `f32` for the masked mean-pool + L2 normalize loops. For 1024-dim
   output that's a 1024-element promotion per record. Modest cost,
   but adds up at high QPS.
3. **Memory bandwidth, not compute, is the bottleneck for bge-large
   on CPU.** Model weights are ~670 MB (fp16) or 1.34 GB (fp32). Once
   the model is mmap'd and the working set is hot in L2/L3 cache,
   compute precision matters less than the per-token attention matmul
   structure, which doesn't shrink with fp16 weights.

## Why small batches still benefit

At 50 docs the per-call setup cost (input tensor allocation, ORT
binding overhead) is a larger fraction of total wall time. fp16
input tensors halve that allocation, giving a clearer relative win.
By 500 docs the per-row compute dominates and the precision benefit
washes out.

## Side benefits worth landing fp16 anyway

Even with a flat throughput delta at 500 docs, fp16 wins on:

1. **Model file size**: 1.34 GB → 669 MB on disk. ~2× faster download
   from ADLS at pod startup. Halves the per-pod model cache footprint.
2. **RAM during inference**: working set halves. On a node running 4
   sessions simultaneously, that's ~2.5 GB saved → can pack 2× the
   pool size at the same memory budget.
3. **Foundation for end-to-end fp16 storage** (see
   `docs/12-design/EMBEDDING_PRECISION_END_TO_END_2026_05_22.adoc`).
   Once the storage path supports fp16, the end-to-end pipeline can
   stay fp16 through WAL, memtable, and HNSW — and the 2× wins on
   storage / bandwidth materialize even if the inference win stays
   modest.

## Recall caveat

The optimum export warned `max diff = 0.0065` between fp32 and fp16
outputs on a reference sentence. That's tiny in absolute terms, but
recall@10 on a real retrieval workload should be measured before
flipping fp16 to default. Recall harness work is part of Phase 4 in
the precision roadmap document.

## Operational recommendation

| Workload | Recommended | Why |
|---|---|---|
| bge-large with cold pods / small batches | **fp16** (`PROXIMADB_EMBED_VARIANT=large` pointing at fp16 export) | +11-19 % at <200 batch; halves model size |
| bge-large with hot pods / large batches | fp16 still preferred for RAM | Throughput is flat but memory is half |
| bge-small | fp32 (no fp16 export tested yet) | Already fast; fp16 conversion overhead may not pay back at 384-dim |
| Production bge-large rollout | fp16 + recall regression test | Validate top-K overlap vs fp32 baseline on real data before defaulting |

## Code changes landed in this iteration

1. `crates/modalities/proximadb-embedding/Cargo.toml` — added `half`
   dependency (gated on `onnx` feature) + `ort/half` feature flag.
2. `crates/modalities/proximadb-embedding/src/models/bge.rs` — output
   extraction now tries f32 first, falls back to f16 and promotes
   element-wise. Same path works for fp32 AND fp16 ONNX models
   transparently — no env var needed; the model file format itself
   determines which extraction path runs.

## Artifacts

- `artifacts/v3_large_fp16good_cpu_{50,200,500}_2026_05_22.json` — raw
  fp16 bench data
- `artifacts/v3_large_cpu_{50,200,500}_2026_05_22.json` — fp32 baseline
  (unchanged from prior bench)
- `docs/12-design/EMBEDDING_PRECISION_END_TO_END_2026_05_22.adoc` —
  end-to-end fp16 storage roadmap (the bigger follow-up)
- This report

## fp16 model staging path

Two-step conversion that works (tested):

```bash
# Step 1: export fp16 ONNX via optimum
optimum-cli export onnx \
  --model BAAI/bge-large-en-v1.5 \
  --library transformers \
  --task feature-extraction \
  --dtype fp16 \
  /tmp/proximadb-models/bge-large-fp16-export

# Step 2: stage at the conventional path the server expects
mkdir -p /var/lib/proximadb/models
cp /tmp/proximadb-models/bge-large-fp16-export/model.onnx \
   /var/lib/proximadb/models/bge-large-en-v1.5.onnx
cp /tmp/proximadb-models/bge-large-fp16-export/tokenizer.json \
   /var/lib/proximadb/models/tokenizer.json

# Step 3: run
PROXIMADB_EMBED_VARIANT=large \
PROXIMADB_EMBED_MODEL_DIR=/var/lib/proximadb/models \
PROXIMADB_EMBED_PROVIDER=cpu \
proximadb-server --config ...
```

The naïve `onnxconverter_common.float16.convert_float_to_float16` path
does NOT work — it leaves a type mismatch at a `Sub` node and ORT
rejects the model with `Type Error: Type parameter (T) of Optype
(Sub) bound to different types`. Use optimum, not raw onnxconverter.
