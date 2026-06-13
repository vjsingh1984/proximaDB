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

## 🚩 Feature Flags
- `unified-facade-routing`: (Default) Directs queries to optimal engines.
- `gpu`: Metal/CUDA acceleration for vector ops.
- `rocksdb`: Optional RocksDB metadata backend.
- `cluster`: Distributed consensus and replication.

## 🚀 Research Frontier & Future Strategic Alignment
ProximaDB is the **primary memory for agentic systems**. Current research focus:

### Phase 5: Agentic Intelligence (Complete)
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
