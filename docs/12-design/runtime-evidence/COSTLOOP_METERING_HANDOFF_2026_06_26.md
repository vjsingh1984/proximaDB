# Handoff — Co-design Cost Loop + Per-Tenant Metering (2026-06-26)

Repo: **`proximaDB`** (this repo). Base branch: **`develop`**.
Read first: `docs/12-design/adr/ADR-030-unified-hotpath-trace-meter-seam.adoc`,
`docs/12-design/adr/ADR-027-unified-storage-and-metering-substrate.adoc`,
`docs/12-design/EXACT_VS_ANN_ROUTING_COST_MODEL_2026_06_26.adoc`,
`CODESIGN_DIMENSIONAL_ARCHITECTURE_2026_06_19.adoc`.

## Theme
Make the co-design cost model real and the per-tenant metering durable: **one hot-path
trace seam feeds two consumers** — the routing cost model (tune) and the per-tenant billing
meters (bill). OSS emits *neutral units*; AnvaiOps prices them (open-core boundary).

## DONE / IN-FLIGHT (this session)
| PR | What | State |
|----|------|-------|
| #369 | ADR-028 index-policy routing + TD-163 flush materialization + TD-165 Defect B + all 4 SDKs | merged |
| #378 | TD-166 filed; TD-165 marked resolved | merged |
| #384 | ADR-030 (design) — unified hot-path trace+meter seam | merged |
| #387 | KRU billing observer + native `compute_ms` | merged |
| #389 | KRU on DataFusion + SST-AXIS | merged |
| #391 | KSU per-tenant storage-snapshot daemon | auto-merge (verify landed) |
| #394 | TD-160 io-trace perf-emission feature gate + billing-never-gated CI guard | auto-merge |
| #396 | TD-164 DrPathBuilder `_metering`/`_trace` paths + reserved-segment guard | auto-merge |

Net: 5 per-tenant dimensions wired (KSU/KRU/KIU/KOU/KEU); cost-model input fed on the hot
read path (gated zero-cost-off); perf/billing boundary CI-enforced; durable-sink **path**
exists.

## REMAINING WORK (pick in this order)

### 1. TD-161 — durable per-tenant `_metering` writer (unblocked by TD-164) — DO NEXT
- **OSS-native writer (no new deps):** extend the KSU snapshot daemon
  (`src/database.rs` ~L176, the `tokio::spawn` interval task) to ALSO persist the per-tenant
  snapshot under the new `DrResolvedPath::metering_subprefix()`
  (`src/storage/trait_components/path_resolver.rs`) via the `FileSystem` trait — JSON or
  PAX, threshold-flush. Aggregation already exists:
  `consumption_metrics::record_storage_snapshot` / `list_collections()`. Mirror the
  `src/metrics/cache.rs` interval task / `MetricsPersistenceLayer` snapshot pattern.
- **OTLP push emitter — DEPENDENCY DECISION REQUIRED:** the tree has `tracing-subscriber` but
  **no** `opentelemetry`/`opentelemetry-otlp`/`tracing-opentelemetry` (see the module doc in
  `src/observability/io_trace.rs` §"OpenTelemetry export"). Today metering is pull-only
  (`/metrics/prometheus`, `src/network/metrics_service.rs`). A periodic OTLP/pushgateway
  emitter is new work gated on adopting those crates — get the dep decision before building.

### 2. Close the routing loop (cost model: observer → gated controller)
- Now that the cost model has real input (TD-158), make it *act*. Scaffolding exists:
  `src/query/route_cost_model.rs` (`route_select_advised`, `install_route_cost_observer`,
  `PROXIMADB_ROUTE_COST_OVERRIDE` flag default-OFF, override warmup/confidence/explore).
  `src/query/compute_scheduler.rs::route_select`.
- **Prove offline first** (mandate #6): replay captured `IoTraceSnapshot`s → show bandit
  regret vs the static heuristic BEFORE flipping the default. Frame as a contextual bandit
  over `(shape_class → backend)` with freshness as a hard constraint.

### 3. TD-115 — cost-based dispatch → DataFusion OLAP wedge (the revenue play)
- `src/network/postgres/relational_pipeline.rs:~251` (`route_select_advised`). DataFusion is
  already wired+ratcheted; the gap is routing the *right* queries there by measured cost and
  lifting the ratchets: `tests/tpch_pgwire_e2e.rs:24` (TPCH_RATCHET=22),
  `tests/tpcds_pgwire_e2e.rs:24` (TPCDS_RATCHET=16). Fix SQL lowering gaps incrementally.

### 4. Evals (mandate #13) — rubric-bearing suites + qa thresholds
- RRF (`tests/multimodal_integration_test.rs` — currently does-it-crash, no rubric),
  Graph-RAG (`tests/graph_rag_integration_test.rs`), Text-to-AQL/RUBICON (does not exist).
  Add nDCG/MRR + trajectory rubrics; gate in `.github/workflows/qa-gate.yml` like the
  `pax-recall-ratchet`.

### 5. Real-S3 cold-cliff validation (the biggest "indicative→real" gap)
- The S3 `FileSystem` backend is NOT compiled (hand-rolled SigV4 incomplete). Finish or
  replace with `aws-sdk-s3`; run the ADR-023 binary-first ladder + IVF Defect-B fix against
  MinIO/Wasabi; measure the cold cliff (beat Turbopuffer's ~444ms→~10ms). Record in
  `BENCHMARK_EVIDENCE.toml` (`CloudLatency` is currently "unverified").

## House rules (non-negotiable)
- One worktree per task off `develop` (`scripts/worktree.sh new <type/topic>`); never edit the
  shared checkout or another worktree.
- No `unwrap/expect/panic` in prod (#4). No agent attribution in commits/PRs (#14).
- Verify the **server binary** builds (`cargo build -p proximadb-server`), `clippy --lib --bins
  -D warnings`, `fmt`. Use `nextest`. Runtime-verify correctness-critical code (#11/#12).
- Storage/wire changes mixed-read-safe + default-OFF (#8). Do **not** merge without explicit
  approval; auto-merge-when-green is fine once approved.
- Offline env: `CARGO_REGISTRIES_CRATES_IO_PROTOCOL=git`. The Bash tool is network-sandboxed —
  use `dangerouslyDisableSandbox: true` for `gh`/network. SDK regen tools live behind the
  per-language `make gen-*-sdk` targets.

## Paste-able continuation prompt
> Continue the co-design cost-loop / metering work in the `proximaDB` repo (branch `develop`).
> Read `docs/12-design/runtime-evidence/COSTLOOP_METERING_HANDOFF_2026_06_26.md` and
> `docs/12-design/adr/ADR-030-unified-hotpath-trace-meter-seam.adoc` first. Next task: **TD-161
> OSS-native durable `_metering` writer** — extend the KSU snapshot daemon (`src/database.rs`)
> to persist the per-tenant `consumption_metrics::record_storage_snapshot` output under
> `DrResolvedPath::metering_subprefix()` via the `FileSystem` trait, mirroring the
> `src/metrics/cache.rs` interval pattern. Surface the OTLP-push dependency decision rather than
> adding `opentelemetry` crates unilaterally. Map the engine/trait boundaries first, runtime-
> verify, then open a PR (do not merge until approved). Apply first-principles + co-design.
