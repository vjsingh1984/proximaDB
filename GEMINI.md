# ProximaDB Agent Guide (GEMINI.md)

This document serves as the foundational mandate for AI agents working on ProximaDB. It defines the project architecture, development standards, and operational workflows.

## 🚀 Project Overview
ProximaDB is a high-performance, cloud-native vector and graph database built in Rust. It combines semantic vector search with native graph traversal for RAG and knowledge graph applications.

[WARNING]
====
**🚨 CRITICAL ARCHITECTURAL PIVOT (2026-06-04) 🚨**
ProximaDB has shifted from a monolithic custom WAL/PAX architecture to an **Intelligent Multi-Engine Routing** system running over decoupled **Object Storage**.

When modifying architecture or execution paths, you MUST adhere to the dual-path mandate:
1. **Data Warehouse/Relational Workloads:** Driven by DataFusion/Polars executing over standard Parquet files managed by Iceberg manifests.
2. **Vector Search/ANN Workloads:** Driven by specialized high-performance engines (SST, HELIX, NOVA) utilizing the custom PAX block format.

You must also strictly enforce SaaS Operational constraints:
- **Path Isolation:** All Object Storage writes must be prefixed by `DrPathBuilder` (`data/{tenant_id}/{namespace_id}/...`). Do not use raw schema locations.
- **Financial Telemetry:** Plumb `TenantContext` to all I/O boundaries to emit accurate billing metrics.
====

[IMPORTANT]
====
**🧬 CO-DESIGN MANDATE (2026-06-19)**
The five physical dimensions — **object storage, network, local disk/cache, compute-per-modality, and
governance/security** — are co-optimized as one system against the **measured trace distribution**,
toward each dimension's **dominant cost term** (for a cloud DB that is I/O round-trips and egress, not
CPU), the way NVIDIA co-designs silicon+compilers and RISC co-designed the ISA+compiler. See
`docs/12-design/CODESIGN_DIMENSIONAL_ARCHITECTURE_2026_06_19.adoc`.

When changing a storage format, reader, codec, cache, or engine you MUST:
1. **Co-design, don't locally optimize** — state which *dimensional cost term* the change moves (a 4×
   codec is irrelevant if you still pay N footer round-trips); optimize across the storage↔compute
   boundary, not within one layer.
2. **Trace before you tune** — justify with a *measured per-query trace*, never a kernel microbench;
   new routes/readers/writers emit the query-scoped I/O trace and the `ComputeScheduler` costs routes
   from measured quantities.
3. **Boundary is the contract** — every I/O boundary carries `TenantContext` (isolation + billing,
   fail-closed) and writes under `DrPathBuilder`; isolation is structural, never a per-query predicate.
4. **Vertical inside, standard outside** — co-design internals freely; expose only at stable seams
   (pgwire, Arrow Flight, Iceberg, REST v2).
5. **Meter every dimension as a TAM surface** — storage (KSU), read/compute (KRU/KIU), network
   outgress (KOU — metered/shipped at pgwire/REST/Flight; distinct from KEU = embedding), cache
   per-tenant; governance as tier entitlement.
====

### Key Technologies
- **Rust (2024 Edition):** Core implementation.
- **Tokio & Axum/Tonic:** Asynchronous runtime and networking (REST/gRPC).
- **Parquet/Arrow:** Columnar storage and memory representation.
- **SIMD:** Hardware-accelerated vector operations (AVX2, AVX-512, NEON).
- **Multi-Engine Storage:** SST, HELIX, VIPER, SWIFT, NOVA, RAPTOR.

## 📂 Project Organization
- `src/`: Core Rust crates (storage, graph, query, vector, api_handlers).
- `src/bin/`: Main binaries (server, bench, migrate).
- `tests/`: Integration, regression, and WAL persistence tests.
- `benches/`: Performance benchmarks (Criterion).
- `clients/`: SDKs for Python (PyO3/C FFI), Go, Java, and Node.js.
- `proto/`: gRPC and internal message definitions (Protobuf).
- `ui/`: Dashboard for monitoring and data exploration.
- `config/`: Configuration templates and environment settings.

## 🏗 Architecture & Data Flow
ProximaDB uses a **Unified Storage Interface** allowing pluggable engines.

1.  **Write Path:** `network/` → `api_handlers/` → `services/` → `WAL` → `Storage Engine` → `Filesystem`.
2.  **Read Path:** `network/` → `api_handlers/` → `services/` → `Search Engine` → `Compute (SIMD)` → `Filesystem`.
3.  **Unified Networking:** Listens on port **5678** (REST + gRPC) by default.

## 🛠 Development Lifecycle

### Build & Run
- `make build` / `make build-release`: Debug vs optimized builds.
- `make server-start`: Start the server in debug mode.
- `cargo run --bin proximadb-server`: Alternative via Cargo.

### Quality & Standards
- `make fmt`: Format code (4-space indent).
- `make clippy`: Run linter (warnings are errors in local dev).
- `make check`: Chain `fmt` + `clippy` + `test`.

### Testing Strategy
- **Unit Tests:** In-line `#[cfg(test)]` modules in `src/`.
- **Integration Tests:** Standalone files in `tests/` for cross-module logic.
- **Python Tests:** `pytest` in `clients/python/tests/`.
- **Command:** `make test` (Full suite), `make test-rust`, `make test-python`.

## 📏 Engineering Mandates (Agent Guardrails)

1.  **Safety First:** NO `.unwrap()`, `.expect()`, or `panic!()` in production code. Use `Result` and `?`.
2.  **Error Handling:** Use `ProximaDBError` for domain logic and `ApiError` for edge/network layers.
3.  **SIMD Awareness:** When modifying vector logic, ensure runtime SIMD detection is preserved (no hard-coded architecture requirements).
4.  **Proto-First:** API changes MUST start in `proto/*.proto` files. Run `cargo build` to regenerate types.
5.  **Documentation:** Use AsciiDoc (`.adoc`) for all technical documentation.
6.  **Performance:** Decisions should be backed by benchmarks (`make benchmark`).
7.  **Token Efficiency:** When running long-output commands (e.g., `cargo check`, `make test`), use `grep` with context flags (e.g., `grep -A 10 -B 5 "error\["`) to filter and display only relevant failure information. Avoid dumping thousands of lines of successful compilation into the context window.
8.  **Reuse Before Reinventing:** Extend existing engines, services, routers, caches, proto contracts, and orchestration layers before adding new abstractions. Directionally aligned work should converge on the canonical path, not fork into a second implementation.
9.  **No Patchwork Architecture:** If a change overlaps an existing concept, refactor the current implementation to absorb it or share the underlying primitive. Avoid proliferating near-duplicate structs, code paths, or APIs for the same behavior.
10. **Distributed-Ready Design:** Favor deterministic behavior, idempotent operations, explicit ownership boundaries, scalable interfaces, and observability-friendly workflows so the code can evolve toward cluster and distributed execution without major rewrites.
11. **Architecture Source Of Truth:** Use `docs/12-design/README.adoc` as the architecture index. Do not duplicate detailed mandates here.
12. **Canonical Data Spine:** New internal/durable contracts use `ProximaRecord` + `ProximaType`/`ProximaValue`; legacy `VectorRecord`, `SqlValue`, `SqlObject`, and protocol DTOs are edge adapters only.
13. **Facades, Not Authorities:** SQL/pgwire, REST/gRPC, Arrow Flight, SDK/embedded, vector, document, graph, and observability APIs lower into xCatalog, canonical records, shared algebra, and canonical WAL.
14. **Stacked Durability:** Durable authority stays in xCatalog + WAL/log/manifest + `ProximaRecord` + policy/RLS. PAX, LSM, columnar, ANN, JSON, graph, observability, Arrow/Parquet/Iceberg/Delta/Hudi are layouts, projections, adapters, or explicit external-authority modes.
15. **Competitive Routing:** OLTP/OLAP/HTAP/MPP routing must be cataloged and explainable via `authority_mode`, `workload_profile`, `storage_specialization`, `freshness_sla`, `compute_route`, `partitioning`, `isolation_profile`, and `policy_boundary`; reject unsafe/stale/lossy routes.
16. **Router Boundary:** Treat routing as a standalone control-plane planner/multiplexer boundary. Route once per plan/fragment/split, emit typed `RoutedExecutionPlan` and unified `EXPLAIN`, then dispatch to leaf executors/readers without per-row route recomputation.
17. **Codegen Guardrails:** Generated code must not create hidden durable authority. New routes/readers/writers/projections/adapters must declare authority mode, policy boundary, freshness behavior/state, repair source, rejected-route reasons, and support maturity before default enablement.
18. **Open-Format Authority:** Iceberg/Delta/Hudi/Parquet paths are interoperability modes. Register them in xCatalog as publications, imports, federated reads, or explicit external-authoritative assets; do not treat files/table logs as Proxima-owned hot authority unless canonical WAL/records own the commit.
19. **Workspace Discipline:** Follow `roadmap/techdebt/WORKSPACE_REFACTOR_PLAN_2026_05_07.adoc`; stable map is `Foundation -> Contracts -> Modality Runtime -> Cross-Model Query Runtime -> Platform Runtime -> Apps/Bindings`; add crates only for real dependency or ownership payoff.
20. **Read Before Touching Architecture:** For records/types/catalog/storage/WAL/query/RLS/open formats/pgwire/Arrow/workspace work, consult the relevant docs in the Architecture References section and cite doc/ADR ids in PRs.
21. **PR Sizing for CI Efficiency (batch related work):** GitHub Actions runners are a shared, finite resource and every PR/push triggers a full CI run — do NOT slice cohesive work into many small PRs. Consolidate related commits (a feature + its tests + docs + adjacent follow-ups) into a single **medium-to-large, "meaty" but coherent** PR: large enough to amortize one CI run, small enough to stay reviewable and single-purpose. Avoid both extremes — trivial one-commit PRs that each cost a full runner cycle, and sprawling unfocused mega-PRs. When you have stacked or independent branches headed to the same base, rebase them onto one branch and open **one** PR rather than N. Always verify locally (`make check` / `cargo clippy` / tests) before pushing so a runner cycle isn't spent catching what you could have caught locally.
22. **Storage-Format Migration (mixed-read-safe, gated):** Any change to an on-disk/wire format, codec, block, segment, or manifest MUST be **mixed-read-safe** — old and new coexist, detected by a version/magic byte, never a flag-day. Ship **default-OFF** (per-collection opt-in or env gate) until baked. Readers/recovery default to the legacy format when the marker is absent — never assume the new format. Gate with **round-trip + recall/quality tests**, not just compile: quantized/lossy formats must hold recall within tolerance of the f32 baseline. Reuse the existing inverse (e.g. `PaxSegmentScanner::read_records`); do not hand-roll a decoder. See `docs/12-design/RABITQ_PAX_SEGMENT_MIGRATION_PLAN_2026_06.adoc` for the canonical phased pattern.
23. **CI/CD Tiering & Security Governance:** CI runs in tiers matched to the `feat -> develop -> qa -> main` promotion flow: **feat->develop LIGHT** (compile + fmt/clippy + layering + proto + panic-policy + security-audit + docs), **develop->qa MEDIUM** (+ unit/integration tests + feature-matrix), **qa->main FULL** (+ coverage + python multi-version + docker + benchmarks + CodeQL). Protected branches gate on the `CI Success` aggregator; **`develop` requires it too** — don't expect to merge red. Heavy scans (CodeQL) run only at the qa->main boundary + weekly. **Never commit secrets/credentials** (even in tests/fixtures — secret-scanning + push-protection are on; CodeQL flags credential literals). Report vulnerabilities via `SECURITY.md` private reporting, not public issues.
24. **Quality Ratchets (correctness beyond compile):** Conformance is ratcheted, not asserted — the TPC-H (22) / TPC-DS (16) pgwire suites and ANN recall@k harnesses carry counts that **only go up**; a change that regresses a ratchet is a failure. ANN/quantization changes are gated against the **f32 recall baseline** (within tolerance). Performance claims are gated on the evidence ledger (`BENCHMARK_EVIDENCE.toml`) — reject kernel microbenchmarks masquerading as end-to-end metrics.
25. **Determinism & Test Hygiene:** Tests must be deterministic and isolated — use **`nextest`** for process isolation; **no non-daemon background threads** (an un-shutdown pool deadlocks the interpreter/runner at exit); pin temp state under `tempdir`, not shared paths. Verify the **server binary builds**, not just `--lib` (a green `--lib` skips `#[cfg(test)]` + feature-gated code). Run perf-sensitive checks on a quiet machine. Known pre-existing flake: `native_volcano_stream_truncates_without_error` (streaming hang) — re-run, don't chase.
26. **Agentic Engineering (Model + Harness):** This repo is developed in the *agentic engineering* mode of Google's *The New SDLC With Vibe Coding* (Osmani, Saboo, Kartakis, 2026): "an agent is a model plus a harness" — and the harness, not the model, is the work (~"10% model, 90% harness"). ProximaDB's harness IS this repo: the rule files (CLAUDE/GEMINI/AGENTS.md = **static context**), per-session **memory**, worktree **sandboxes**, the **guardrails/hooks** (panic-policy, layering, tenant-path, OSS-boundary, secret-scan, the CI tiers), multi-agent **orchestration**, and **observability** (Prometheus/billing). The differentiator of this mode is "not whether you use AI, it is how outputs get verified": production work runs the disciplined end — spec/ADR → tests **and evals** → CI gates — never vibe-coding (reserve that for throwaway spikes). Since "AI turns implementation from writing into reviewing", review generated code for its failure modes — **hallucinated dependencies/APIs, plausible-but-wrong logic, silent duplication of an existing primitive** — not just style. Keep these rule files **lean and high-signal** (static context is paid on every call); push detail to on-demand docs/skills (progressive disclosure).
27. **Evals for Non-Deterministic Surfaces:** Per the tests-vs-evals split — "tests cover the deterministic parts; evals cover the parts that aren't deterministic" — any ranked, generated, or model-driven surface (ANN recall@k, hybrid/RRF ranking, embedding/semantic relevance, RAG / Graph-RAG retrieval, Text-to-AQL / RUBICON plans) MUST ship an **eval suite with a real rubric**, covering both **output** (is the result correct) and **trajectory** (was the route / tool-calls / plan sound) — "set the bar at the eval, not the demo." Gate shared/agentic workflows on **eval thresholds** like test coverage (a regression fails CI; ties to the Quality-Ratchet mandate), and **version the prompts + eval suites in the PR** that changes the behavior. Watch production for drift.
28. **Commit/PR Hygiene (no agent attribution):** Do NOT add AI-agent authorship to commit messages or PR bodies — no `Co-Authored-By: <AI>` trailers, agent model/product signatures (e.g. "Gemini"/"Claude Code"), agent no-reply emails, or "Generated with …" footers. The human drives the code; `scripts/check_no_agent_attribution.py` enforces this and rejects the commit/PR otherwise. (Mentions of CLAUDE.md/GEMINI.md or the model APIs in *content* are fine — only authorship attribution is blocked.)

## 🚩 Feature Flags
- `unified-facade-routing`: (Default) Directs queries to optimal engines.
- `gpu`: Metal/CUDA acceleration for vector ops.
- `rocksdb`: Optional RocksDB metadata backend.
- `cluster`: Distributed consensus and replication.

## 🚀 Research Frontier & Future Strategic Alignment
ProximaDB is the **primary memory for agentic systems**. Current research focus:

### Phase 5: Agentic Intelligence & MLOps (In Progress)
- **MLOps & Model Management:** ProximaDB serves as the single source of truth for the MLflow landscape. It natively maps MLflow experiments, model registries, and artifacts to `xCatalog`, enabling tight integration between data engineering (DataFusion) and model serving/training.
- **PySpark-style Data Engineering:** A Rust-backed, Python DataFrame API (via PyO3) that compiles down into the same DataFusion/Rust logical plan, providing scalable distributed execution for ML/Monte Carlo workloads without JVM overhead.
- **RUBICON / AQL (Stonebraker Design):** Auditable agentic query plans and Text-to-AQL.
- **Modular Graph RAG (RGL):** Dynamic subgraph construction and retrieval.
- **Projection-Based Fusion (B5):** Speed/diversity tradeoff (score or vector).

### Phase 6: Active Memory & Collective Reasoning (2026+)
- **True Memory Architecture:** Verbatim event preservation with Encoding Gates (Novelty/Salience/Error).
- **L-RAG (Lazy Loading):** Entropy-gated retrieval to reduce context noise and latency.
- **Memanto (Typed Recall):** High-precision retrieval via 13 standardized semantic categories.
- **Agentic Hybrid Reference Architecture:** Plan–Retrieve–Evaluate loops with multi-agent orchestration.

## Architecture References
- `docs/12-design/README.adoc` - architecture index.
- `roadmap/MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc` - record/type/algebra/storage/RLS internals and sticky ADRs.
- `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc` - stacked durability and modality convergence.
- `docs/12-design/COMPETITIVE_OLTP_OLAP_MPP_TRAJECTORY_2026_05_20.adoc` - OLTP/OLAP/HTAP/MPP trajectory and route knobs.
- `docs/12-design/RELATIONAL_STORAGE_FORMAT_AND_INTEROPERABILITY_2026_05_19.adoc` - PAX/MVCC/open-format storage shape.
- `docs/12-design/RELATIONAL_PGWIRE_DML_COMPUTE_BLUEPRINT_2026_05_20.adoc` - active pgwire DML/compute tracker.
- `docs/12-design/OPEN_FORMAT_CATALOG_2026_05_17.adoc` - open table authority modes and catalog contracts.
- `docs/12-design/adr/ADR-004-unified-explain-contract.adoc` - unified EXPLAIN and route/write-plan explanation contract.
- `docs/12-design/adr/ADR-007-iceberg-rest-catalog-server.adoc`
- `docs/12-design/adr/ADR-008-oltp-catalog-backends.adoc`

---

## 🔍 Quick Reference
- **Default Port:** 5678 (Unified), 5679 (gRPC), 5433 (PostgreSQL wire), 5680 (Arrow Flight).
- **Default Data Path:** `/tmp/proximadb/`.
- **Health Check:** `curl http://localhost:5678/health`.
- **Log Levels:** `RUST_LOG=proximadb=debug`.

### Arrow Flight SQL (high-throughput integration for Gemini pipelines)

```python
import pyarrow.flight as flight

client = flight.FlightClient("grpc://localhost:5680")
# Execute SQL query
descriptor = flight.FlightDescriptor.for_command(
    b'SELECT id, tenant_id, props, labels FROM my_collection LIMIT 1000'
)
info = client.get_flight_info(descriptor)
reader = client.do_get(info.endpoints[0].ticket)
table = reader.read_all()  # returns pyarrow.Table with ProximaRecord schema
```

For multimodal records (text + embedding), project `embedding_{model_id}` columns as Arrow `list<float32>`.  Use the Iceberg table properties `proximadb.index.{col}.dim` and `.type` to discover index configuration before building embedding queries.
- **Iceberg Catalog URI:** `http://localhost:5678/iceberg/v1`.
