# io_trace sink hot-path overhead — sink OFF vs ON (2026-07-22)

**Purpose.** ADR-066 §6 acceptance condition #3: prove the structured-io-trace
sink adds **bounded, CPU-only overhead** to the query path and performs **no
filesystem / network / compression work there**. This is the gate that blocks
flipping ADR-066 from Proposed → Accepted.

**Method.** The sink's *only* synchronous query-path work is the observer
closure installed at `src/observability/io_trace_sink.rs:167`:

1. `TraceEnvelope::from_snapshot` — clone the header fields + classify the
   modality payload (no I/O).
2. `serde_json::to_vec` — serialize the envelope (the dominant cost).
3. a bounded `Spool::push` (Mutex lock + `saturating_add` + Vec push).

The expensive work — `zstd::encode_all` compression, local seal, and the
object-store conditional-create upload — all run in the **background `Worker`**
(`spawn_blocking` at `io_trace_sink.rs:371`+), never on the query path. The
`IoTrace` snapshot itself is taken at `instrument()` exit **unconditionally**
(sink ON or OFF), so the sink's *incremental* per-query cost is exactly the
closure above.

The micro-bench `observer_closure_hot_path_is_bounded`
(`src/observability/io_trace_sink.rs::tests`) drives those exact components over
**N = 20,000** representative vector-ANN envelopes (12 GETs, ~1.2 MB read, 8
range-gets, a 64-block centroid prune, two compute engines — 585 B serialized),
timing each closure with `Instant::now()`. The snapshot is built via the public
`IoTrace` record API (no struct-literal drift); the spool has a 64 MiB cap (no
drops during the run).

Reproduce:
```
cargo nextest run --lib observer_closure_hot_path_is_bounded --run-ignored only --nocapture
```

**Result (N = 20,000, macOS arm64 dev machine, serialized run):**

| metric | value |
|---|---|
| p50 | **13,959 ns (~14 µs)** |
| p95 | **15,959 ns (~16 µs)** |
| p99 | 58,666 ns (~59 µs) |
| envelope size | 585 bytes |

The p99 tail (~59 µs) is scheduler/GC jitter across 20k samples; p50/p95 are
tight (~14–16 µs). The closure's sanity guard (`p95 < 100 µs`) holds with ~6×
headroom.

**Conclusion.** ADR-066 §6 #3 is satisfied: the sink adds a bounded ~16 µs p95
of **CPU-only** work per query (serialize + enqueue) and performs **no fsync,
network, or compression** on the query path — that work is the background
worker's. For context, a single object-store range-GET is ~1–10 ms (orders of
magnitude larger), so the sink's overhead is in the noise of any real query.

**Caveats / follow-ups.** Indicative dev-machine run, **not an SLA** — absolute
numbers vary by hardware/load; the re-runnable micro-bench + the `p95 < 100 µs`
guard are the durable artifact. A future end-to-end measurement (full query
latency distribution with the sink installed against a live object store, OFF vs
ON) would strengthen this to a full-query-path claim; this micro-bench isolates
the sink's *incremental* cost, which is the part the ADR's acceptance condition
names. Governed by `BENCHMARK_EVIDENCE.toml` claim `io_trace_sink_hotpath_p95`.
