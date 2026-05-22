# Cross-surface write benchmarks, 2026-05-22

Goal: identify the root cause of the "async slower than sync" pattern observed
in the embedded path by validating against a running `proximadb-server` over
every wire protocol (REST v1, REST v2, **gRPC v1, gRPC v2**, pgwire SQL/UQL,
Arrow Flight) at the same scale (10 000 rows × 64 dim × 3 runs).

Server: `target/release/proximadb-server --config config/minimal.toml`, ports
5678 (REST/gRPC unified), 5679 (dedicated gRPC), 5433 (pgwire),
5680 (Arrow Flight).

Bench scripts:
- `clients/python-embedded/benchmarks/write_sync_async_modalities.py` — embedded
- `clients/python-embedded/benchmarks/server_writes_all_surfaces.py` — REST/pgwire/Flight
- `clients/python-embedded/benchmarks/server_writes_grpc.py` — gRPC v1/v2

Raw JSON:
- `artifacts/sync_vs_async_writes_10k_2026_05_22.json` — embedded
- `artifacts/server_all_surfaces_10k_2026_05_22.json` — REST/pgwire/Flight
- `artifacts/server_writes_grpc_10k_2026_05_22.json` — gRPC

## Headline numbers (10 k rows × 64-dim, 3 runs, median ops/s)

Sorted by sync throughput; n/a means the client library has no async API.

| Surface | Shape | Sync ops/s | Async ops/s | Async/Sync |
|---|---|---:|---:|---:|
| **Arrow Flight** | DoPut, one call | **255 535** | n/a | — |
| **gRPC v1** | VectorService.VectorBatch | **234 793** | 123 023 | **0.52×** |
| **gRPC v2** | ProximaRecordService.InsertRecords | **192 167** | 114 369 | 0.60× |
| **Embedded** (in-process) | vector.insert_numpy | 116 800 | 147 224 | **1.26×** |
| **Embedded** (in-process) | record_wire ProximaRecord | 103 158 | 122 968 | 1.19× |
| **pgwire** | SQL INSERT, 1 000-row chunks | 39 538 | **75 913** | **1.92×** |
| **pgwire** | UQL `INSERT` via SQL | 40 471 | n/a (same path) | — |
| **REST v2** | `/records/batch` ProximaRecord JSON | 31 265 | 31 409 | 1.00× |
| **REST v1** | `/vectors/batch` proto JSON | 29 060 | 30 623 | 1.05× |

## Major findings

### 1. gRPC sync is 7-8× faster than REST sync

The whole REST-vs-gRPC story collapses to one number — gRPC `VectorBatch` sync
hits **234 793 ops/s** vs REST v1 sync at **29 060 ops/s**. Same handler
underneath (`handle_record_batch_for_tenant`), same storage path. The only
difference is the wire encoding:

| Layer | gRPC | REST |
|---|---|---|
| Wire format | Protobuf binary | JSON text |
| Transport | HTTP/2 multiplexed | HTTP/1.1 (mostly) |
| Per-row encoding | tight binary, length-prefixed | UTF-8, numeric formatting |
| Per-row server decoding | streaming proto | full-buffer JSON parse |

JSON encoding/decoding is the dominant cost on REST, full stop. **For
write-heavy workloads, gRPC should be the default**.

### 2. Async loses on gRPC (and embedded) but wins on pgwire

| Path | Async/Sync | Why |
|---|---:|---|
| pgwire (8 separate connections) | **+92 %** | 8 server-side sessions; only LSM memtable contends |
| Embedded vector @ 10 k | **+26 %** | GIL released; PyO3 numpy→Vec converts in parallel |
| REST v1 / v2 (aiohttp gather) | ~0 % | HTTP fan-out savings = server-side serialization cost |
| gRPC v1 / v2 (grpc.aio gather) | **−45 %** | Per-call protobuf encode + HTTP/2 frame is small; fan-out cost dominates |
| Embedded `ingest_metrics` | **−4×** | Sub-µs op |

**This validates the root-cause hypothesis**: the bottleneck is the
storage-engine write lock and the per-call setup overhead — not the wire
protocol itself. When per-call work is small (gRPC binary frames are ~25 KB
for 1 250 rows), the asyncio fan-out cost dominates. When per-call work is
embedding-heavy or parser-heavy (pgwire SQL parsing 100 KB statements), async
parallelizes that work and wins.

### 3. Wire-protocol ranking (sync, 10 k rows × 64-dim)

| Surface | Sync ops/s | vs embedded | Wire cost |
|---|---:|---:|---:|
| Arrow Flight DoPut | 255 535 | **2.19×** | Columnar Arrow IPC, zero JSON |
| **gRPC v1** VectorBatch | 234 793 | **2.01×** | **Protobuf binary, HTTP/2** |
| **gRPC v2** InsertRecords | 192 167 | **1.65×** | Protobuf, but more optional ProximaRecord fields |
| Embedded vector.insert_numpy | 116 800 | 1.00× | No network |
| Embedded record_wire | 103 158 | 0.88× | + ProximaRecord materialization |
| pgwire SQL DML | 39 538 | 0.34× | SQL parsing dominates |
| pgwire UQL | 40 471 | 0.35× | Same path |
| REST v2 records batch | 31 265 | 0.27× | JSON encoding |
| REST v1 vector batch | 29 060 | 0.25× | JSON + proto enum remap |

**gRPC and Arrow Flight both beat embedded** because they bypass the Python
PyO3 boundary (`numpy.tolist()` + per-row Vec allocation) that the embedded
path pays. Arrow Flight wins over gRPC because of the columnar transfer (one
Arrow IPC frame for the whole batch vs gRPC's row-by-row protobuf).

### 4. v1 vs v2 ranking

For each protocol, v2 is the canonical ProximaRecord shape, v1 is the legacy
shape. Per-protocol delta:

| Protocol | v1 sync | v2 sync | v2 vs v1 |
|---|---:|---:|---|
| REST | 29 060 | 31 265 | **+7.6 %** |
| gRPC | 234 793 | 192 167 | **−18 %** |

REST v2 is slightly faster than v1 because the request shape skips the proto
enum remap path. gRPC v2 is slower than v1 because `ProximaRecord` has many
optional fields (`props`, `text_fields`, `partition_values`,
`custom_metadata`, `source_type`, `created_by`, `updated_by`) that the proto
encoder must walk per-record even when empty. **For pure-vector ingest,
gRPC v1 is the fastest gRPC path**; for typed records, v2 is the right
contract.

## Why are REST sync and async identical (and what we did NOT test)

The user asked: "I believe embedding will take time if we send document on sync
path as compared to async — or are we simply testing supply vectors via SDK?"

**Correct — we tested pre-computed vectors only**. None of the surfaces
exercised in this run trigger the `EmbeddingService` path:

- `/api/v1/vectors/batch` — takes `vector: [f32; D]` directly
- `/api/v2/collections/{}/records/batch` — takes `vector: [f32; D]` directly
- `gRPC VectorService.VectorBatch` — takes `vector: repeated float`
- `gRPC ProximaRecordService.InsertRecords` — takes `vector: repeated float`
- Arrow Flight DoPut — takes Arrow `list<float32>`
- pgwire `INSERT ... VALUES ('id', '[0.1, 0.2, ...]')` — takes vector literal

The path that DOES auto-embed is `/api/v3/documents`
(`src/network/rest/v3/documents.rs:214` → `embed_text_only_records`). On that
path, sync would block per request awaiting model inference (10-50 ms per
batch on local sentence-transformers, hundreds of ms on remote APIs).
**Async would dominate sync there because each concurrent request overlaps
an independent model call.**

This was not tested because the server we ran (with `config/minimal.toml`)
has no LLM section configured, and the `config/config.toml` couldn't load
into the May-20 binary due to a `semantic_cache_enabled` field validation
mismatch.

To validate this hypothesis end-to-end, the next step is:
1. Add the missing config field or rebuild the server, and
2. Re-run with `POST /api/v3/documents` sending text-only records (no
   `vector` field) at scale 200 docs (because local embedding throughput
   caps at ~20-50 docs/sec on this hardware).

## "Is async more reliable than sync?" — No

All surfaces call into `vector_operations_service.insert_batch()` and wait
for the WAL append before returning. **Sync and async have identical
per-byte durability**. Async simply makes more, smaller WAL appends —
finer atomic-commit boundaries, not safer writes.

## Recommendations

| Workload | Use | Why |
|---|---|---|
| Bulk vector ingest, you control the client | **Arrow Flight DoPut** | 2.2× faster than embedded |
| Server-mediated vector ingest, typed client | **gRPC v1 VectorBatch** (legacy) or **gRPC v2 InsertRecords** (canonical) | 6-8× faster than REST |
| Browser/scripting/curl-friendly | REST v2 | Cleaner contract than v1, same perf |
| Auto-embedding workflow | REST v3 documents (use async) | Embedding cost dominates; async wins |
| OLTP SQL clients, multiple connections | pgwire async (2× win) | Per-connection parser parallelizes |
| Embedded in-process bulk ingest | sync, large batches (≥1 250 rows) | Async only helps for dense numpy paths |
| Embedded async | Only when chunks are dense and ≥1 250 rows | Otherwise asyncio fan-out wastes more than it saves |

## Per-async-mechanism summary

| Async mechanism | Real concurrency? | Server-side serialization? | Net effect |
|---|---|---|---|
| `asyncio.to_thread` + 8 workers (embedded) | Yes (GIL released) | Storage write lock | Mixed: +26 % for dense, −4× for cheap ops |
| `aiohttp.gather` + 8 conns (REST) | Yes | axum handler queues + storage lock | ~0 % at any scale |
| `grpc.aio.gather` + 8 conns (gRPC) | Yes | tonic handler queues + storage lock | −45 % (fan-out cost dominates fast call) |
| `psycopg2` over 8 threadpool conns (pgwire) | Yes | Per-connection parser, then storage lock | +92 % (parser parallelism is the win) |

## Tasks and artifacts

- Final tasks all complete (#1–#12).
- Scripts: `write_sync_async_modalities.py`, `server_writes_all_surfaces.py`,
  `server_writes_grpc.py`, `compare_against_baseline.py`.
- JSON: `sync_vs_async_writes_isolated_2026_05_22.json` (1k),
  `sync_vs_async_writes_10k_2026_05_22.json`,
  `server_all_surfaces_10k_2026_05_22.json`,
  `server_writes_grpc_10k_2026_05_22.json`,
  `baseline_modalities_2026_05_22.json`.
- Reports: `embedded_writes_2026_05_22_report.md`, this file.

## Open follow-ups

- v3 documents path with active EmbeddingService — to confirm async dominates
  when per-request work is embedding-heavy.
- gRPC streaming (`BatchWriteStream`) — single long-lived stream vs unary
  `InsertRecords`; should beat unary on connection-setup cost.
- Cross-machine LAN measurement — current numbers are all loopback.
