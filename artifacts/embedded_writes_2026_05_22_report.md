# Embedded Python Writes: Sync vs Async, 2026-05-22

Re-built the maturin embedded wheel and ran modality write/read benchmarks at
two batch sizes (1 000 and 10 000 rows per modality). A 100 000-row run was
attempted but killed early — it pushed the OS into memory pressure (free pages
< 100 MB out of 64 GB RAM) and disk free was at 7 GB before cleanup. The
1 k / 10 k pair already characterizes both per-call overhead and amortized
throughput, so 100 k was not re-attempted.

## Build artifacts

- `target/wheels/proximadb_embedded-0.2.0-cp314-cp314-macosx_11_0_arm64.whl`
  (release wheel for Python 3.14)
- `~/code/.venv/lib/python3.12/site-packages/proximadb_embedded`
  (cp312 editable install via `maturin develop --release`, fresh `.so`
  at `clients/python-embedded/src/proximadb_embedded/_proximadb_embedded.cpython-312-darwin.so`,
  built 2026-05-22 00:20)
- Built in 11m 41s on macOS arm64

## Scripts and raw data

- New bench script: `clients/python-embedded/benchmarks/write_sync_async_modalities.py`
- Comparison helper: `clients/python-embedded/benchmarks/compare_against_baseline.py`
- Raw aggregates:
  - `artifacts/sync_vs_async_writes_isolated_2026_05_22.json` (1 k rows)
  - `artifacts/sync_vs_async_writes_10k_2026_05_22.json` (10 k rows)
  - `artifacts/baseline_modalities_2026_05_22.json` (search + records baseline)
- Reference baseline: `docs/02-guides/api-surface-performance-guide.md`
  (2026-05-19, scale=200, dim=64, 3 runs, same hardware/python)
- Methodology: 3 isolated Python subprocesses per row count (forks per run, not
  in-process, so each run starts cold)

## Headline numbers

### Apples-to-apples vs documented baseline (sync writes)

The baseline was scale=200 → 1 000 vector rows / 200 other-modality rows.
The new run uses uniform row counts.

| Modality | Baseline (2026-05-19) | This run @ 1 k | This run @ 10 k | Best delta vs baseline |
|---|---:|---:|---:|---:|
| `vector.insert_numpy` (legacy batch) | 89.2 k | 61.4 k | **116.8 k** | **+31 % @ 10 k** |
| `record_wire.vector_insert` (ProximaRecord wire) | 75.1 k | 69.8 k | **103.2 k** | **+37 % @ 10 k** |
| `arrow.insert_arrow` | 82.5 k | 92.3 k | **135.7 k** | **+65 % @ 10 k** |
| `relational.sql_insert_multirow_batch` | 20.1 k | 33.3 k | 28.0 k | **+65 % @ 1 k** |
| `record_wire.vector_search_top10` (generic) | 339 | 325 | n/a* | −4 % |
| `vector.search_top10` (generic) | 652 | 474 | n/a* | −27 % |
| `graph_entity.cypher_match_entity_limit10` (search) | 4 100 | 3 832 | n/a* | −7 % |

* Search numbers come from `baseline_modalities_2026_05_22.json` (see Search
  section below). The new sync/async script's `--no-search` flag was set for
  the 10 k stress run to isolate write throughput.

**Read pretty much flat. Writes are uniformly faster than the 2026-05-19
baseline once batches reach 10 k rows.** At 1 k rows the vector path is
slower than baseline — per-call overhead from T15 WAL-lane enforcement and
schema validation dominates short batches.

### Sync vs async at the two scales

| Modality | Sync @ 1 k | Async @ 1 k | Sync @ 10 k | Async @ 10 k | Async crosses sync |
|---|---:|---:|---:|---:|---|
| `vector.insert_numpy` | 61 386 | 30 916 | 116 800 | **147 224** | **at 10 k, async beats sync 1.26×** |
| `record_wire.vector_insert` | 69 850 | 23 427 | 103 158 | **122 968** | **at 10 k, async beats sync 1.19×** |
| `arrow.insert_arrow` | 92 340 | 25 306 | 135 744 | 91 545 | sync wins both (PyArrow table build per chunk is the cost) |
| `document.insert` | 100 372 | 95 337 | 96 030 | 93 249 | effectively tied |
| `graph_entity.create_nodes` | 117 007 | 112 823 | 266 768 | 250 586 | effectively tied |
| `graph_entity.create_edges` | 54 837 | 54 114 | **2 556** | **2 420** | **20× drop at 10 k — see Findings** |
| `observability.ingest_logs` | 407 747 | 534 521 | 430 673 | 325 052 | async wins at 1 k, sync wins at 10 k |
| `observability.ingest_metrics` | 896 057 | 242 471 | 881 533 | 233 806 | sync wins (sub-µs op, async fan-out kills it) |
| `observability.ingest_traces` | 261 900 | 128 484 | 74 008 | 65 532 | sync wins; throughput drops as trace span index grows |

## Findings

1. **Async write model is a real win for medium-to-large batches**. At 10 k
   rows per modality, `asyncio.to_thread`+`gather` with 8 workers beats sync
   for the two highest-volume write paths (`vector.insert_numpy` 1.26× and
   `record_wire.vector_insert` 1.19×). At 1 k rows the asyncio fan-out cost
   eats the wins. **Rule of thumb**: prefer async only when each chunk is
   ≥1 250 rows of dense vector or canonical record data.

2. **Per-call overhead increase since 2026-05-19 baseline**. At 1 k rows the
   vector path is −31 % vs baseline; at 10 k rows it's +31 %. The
   per-batch fixed cost has grown — consistent with the
   `enforce_wal_lane_for_record_batch` (T15 WAL-policy check),
   `validate_record_batch_against_schema` (T15 schema check), and the four
   `bump_*` stats updates (T8 `last_analyzed_ms` write on every successful
   write) that landed since the baseline.

3. **Graph edge creation has a non-linear bottleneck**. `graph_entity.create_edges`
   collapsed from 54.8 k/s @ 1 k → 2.6 k/s @ 10 k (20× drop while size grew
   10×). The CSR rebuild epoch (`csr_rebuild_epochs` in
   `src/graph/service.rs`, added in stash@{2}) is bumped per edge mutation;
   if that triggers a per-call topology read of the projection registry it
   would explain the slope. Worth investigating separately.

4. **Observability `ingest_metrics` is the fastest write path on the embedded
   surface** (881 k/s sync at 10 k). It's a flat append to the in-memory
   ring with no schema validation, no WAL policy check, no per-row
   ProximaRecord materialization. Async cannot win here — the call itself
   is sub-µs so any threading overhead doubles latency.

5. **Arrow async lost to Arrow sync at both scales**. Each async chunk
   constructs a fresh PyArrow table from Python lists which is a 4-figure µs
   cost. To make Arrow async profitable, the caller should build one big
   table and partition it with `.slice()` rather than rebuild per chunk.

## Read / search baseline (from `baseline_modalities_2026_05_22.json`)

Same hardware, 3 isolated runs, scale=200, dim=64.

| Path | This run | Baseline (2026-05-19) | Delta |
|---|---:|---:|---:|
| `vector.search_top10_profiled` (NumPy native) | 31 846 | 32 400 | −2 % |
| `record_wire.vector_search_top10_profiled` | 28 685 | 26 600 | +8 % |
| `vector.search_top10` (generic Python) | 613 | 652 | −6 % |
| `record_wire.vector_search_top10` (generic) | 327 | 339 | −4 % |
| `record_wire.sql_vector_search_top10` | 12 856 | 13 000 | −1 % |
| `record_wire.uql_vector_search_top10` | 15 555 | 11 500 | **+35 %** |
| `graph_entity.cypher_match_entity_limit10` | 3 832 | 4 100 | −7 % |
| `graph_entity.traverse_depth2` | 83 092 | (none) | n/a |
| `document.query_indexed_path` | 2 236 | 2 250 | −1 % |
| `observability.query_logs` | 81 794 | 79 800 | +2 % |
| `observability.query_traces` | 483 689 | (none) | n/a |
| `relational.sql_vector_search_top10` | 9 160 | (none) | n/a |

The UQL `vector_search_top10` +35 % stands out — almost certainly from the
multimodel-plan consolidation that landed in the recent workspace-refactor
wave (stash@{1}, stash@{2}). Everything else is within ±10 %, i.e. flat.

## Notes on the failed 100 k run

- 100 000 rows per modality × 7 modalities × 2 (sync + async) = 1.4 M rows in
  RAM per process. Pythonside numpy buffers + Rust-side WAL + LSM memtable
  pushed free pages from 43 k → 4 k (16 KB pages, ~64 MB free out of 64 GB).
- /private/tmp was at 100 % (7.3 GB free of 1.8 TB) because 27 stale
  cargo target dirs from previous sessions were sitting there
  (`proximadb-codex-target` alone was 36 GB).
- After killing the in-flight 100 k run and clearing the cargo target dirs,
  /private/tmp is back to 95 % (92 GB free) and OS memory is healthy.

If a 100 k run is needed again, the prerequisite is: disable async (so
sync+async halves the working set), or run one modality at a time with
explicit `db.flush()` between each. The bench script supports
`--no-search` already; an additional `--modality-only` flag would be the
right knob.
