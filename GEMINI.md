# ProximaDB Agent Guide (GEMINI.md)

This document serves as the foundational mandate for AI agents working on ProximaDB. It defines the project architecture, development standards, and operational workflows.

## 🚀 Project Overview
ProximaDB is a high-performance, cloud-native vector and graph database built in Rust. It combines semantic vector search with native graph traversal for RAG and knowledge graph applications.

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
11. **Follow The Workspace Skeleton:** Use `roadmap/techdebt/WORKSPACE_REFACTOR_PLAN_2026_05_07.adoc` as the active architectural mandate for workspace boundaries and migration sequencing.
11a. **Follow The Multi-Model Overhaul Spec:** Treat `roadmap/MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc` as the authoritative specification for record format, type system, modality boundaries, logical algebra, query layer, and security placement. It supersedes prior multi-model review docs for go-forward design decisions.
11b. **Sticky ADRs:** The ADRs in §12 of the overhaul spec are immutable in normal turns. Do not relitigate them. If a turn proposes a deviation, name the ADR id and justify against the convergent research (spec §2) and gap analysis (spec §1.2) explicitly.
11c. **Sticky Design Pillars:** New code touching records, types, indexes, query, storage, security, or distributed architecture must respect:
    - One record envelope (`ProximaRecord`, spec §3) — no parallel record shapes per modality.
    - One scalar type system (`ProximaType`/`ProximaValue`, spec §4) — Decimal/TimestampTz/Uuid/Json/Jsonb/Vector are first-class wire types; `SqlValue` is legacy.
    - Canonical model first across every surface — new foundation, modality, storage, query, catalog, SDK, embedded, REST, gRPC, Arrow Flight, pgwire, and SQL-lowering contracts use `ProximaRecord` plus `ProximaType`/`ProximaValue`; legacy v1 `VectorRecord`, `SqlValue`, `SqlObject`, and vector metadata maps are deprecated migration artifacts, not target API contracts.
    - Protocol adapters are not durable authorities — SQL/pgwire, REST, gRPC, Arrow Flight, Mongo-like document APIs, Gremlin/Cypher/PGQ graph APIs, Neptune/Titan-style compatibility, SDKs, and embedded helpers lower into `ProximaValue`, `ProximaRecord`, xCatalog schema/variation metadata, or the shared logical plan. Do not let protocol request/response types define storage, type, RLS, WAL/recovery, or catalog semantics.
    - Cataloged schema modes — strict relational tables, flexible document/graph/entity collections, schema-on-write variation registration, and schema-on-read projection behavior are xCatalog/table capabilities. Insert/upsert paths across SQL, REST, gRPC, Arrow, and embedded mode share type validation/coercion before WAL/storage.
    - ADR-009 schema/API convergence — schema-on-read external files/tables, schema-on-write OLTP loads, REST/gRPC, Arrow Flight, pgwire/SQL, SDKs, and embedded mode are surfaces over xCatalog plus `ProximaRecord`; breaking changes are acceptable when they remove vector-only API authority.
    - Typed Semantic Memory (TD-055) — 13 standard categories (`Fact`, `Goal`, `Preference`, `Decision`, etc.) for high-fidelity agent recall and conflict resolution.
    - Three-layer storage (spec §5) — record (PAX) + topology (CSR/COO) + vector index (HNSW relationship table) behind one buffer pool.
    - One logical algebra (spec §7) — `Filter, Project, Sort, Limit, Aggregate, Union, Join, HybridTraverse, PatternMatch, CrossModelJoin, VectorTopK, ModulationOp, MatrixOp, SemanticJoin, ModelConvert`.
    - Engine-level RLS (spec §8) — `tenant_id`/`permitted_principals` are record fields; no application-layer-only tenant filtering.
    - Predicate-aware HNSW (spec §6.1) — γ-expanded ACORN construction + NaviX adaptive-local search + per-modality shards + PAX co-location.
11d. **Phase A-G Roadmap:** The Phase A-G sequencing in spec §11 is the multi-model overhaul roadmap. New TD entries and roadmap updates must reference Phase A/B/C/D/E/F/G/H or explain why the work falls outside that envelope.
11e. **Best-In-Class Mandate:** The overhaul spec exists to make the system scalable (disaggregated runtime, three-tier cache), maintainable (one envelope, one type system, ADR-tracked decisions), extensible (operators added once, surface languages cheap, symmetric modality crates), and robust (engine-level RLS, atomic metadata+vector writes, subschema-checked schema evolution). Every change should preserve or strengthen these axes.
11f. **Data and AI Platform Anchor:** Treat `docs/12-design/DATA_AI_PLATFORM_ARCHITECTURE_ANCHOR_2026_05_12.adoc` as the product/platform anchor for competitive moat, xCatalog, pgwire compatibility, storage/compute/streaming sidecars, branchable state, agentic runtime, MLOps/MLflow/AutoML, Ray/Flink/Kafka/Kinesis support, and the phasewise developer-to-enterprise roadmap. Platform changes must map to an anchor phase and plane while preserving the multi-model overhaul spec for internals.
11g. **Relational/Document/Graph Convergence:** Treat `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc` as the mandate for document and graph storage evolution. `DocumentService`, graph services, Cypher/PGQ, document APIs, and SDK modality helpers are facades over canonical `ProximaRecord`/relational storage plus rebuildable projections. Do not add separate durable document/graph semantics, record envelopes, WAL/recovery paths, RLS enforcement, or compaction rules. JSON path indexes, full-text indexes, adjacency tables, CSR/COO, and HNSW are adaptive projections/access methods unless an accepted ADR proves otherwise.
11g.1. **Load The Mandate For Relevant Turns:** Before making or reviewing changes in records, storage, document, graph, vector, observability, query lowering, indexing, catalog, WAL/recovery, or workspace boundaries, read `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc` into the current session context and keep the mandate available for the turn. Do not rely on stale recall of the architecture decision.
11g.2. **No Legacy Value-System Fallback:** New modality contracts must not use legacy v1 `SqlValue`/`SqlObject` as their internal shape. Convert old protocol/service types at the edge and keep foundation contracts on `ProximaRecord`/`ProximaValue` with v2-compatible rich datatypes.
11h. **Stacked Durability With Adaptive Projections:** Do not reframe the architecture as "generic row store vs separate durable engines." The accepted trajectory is one durable semantic spine (`ProximaRecord`/`ProximaType`, xCatalog, WAL/log/manifest, tenant/RLS, version/time, provenance, retention policy) plus specialized physical structures: LSM/PAX/columnar layouts, adjacency tables, CSR/COO, graph lake projections, ANN fragments, time-series compression blocks, trace indexes, and event streams. These structures must be cataloged, rebuildable where applicable, benchmarked, and tied to explicit freshness/rebuild semantics.
11h.0. **Internal Spine, Not External-RDBMS Default:** Do not interpret stacked durability as delegating ProximaDB's hot durable core to PostgreSQL, DuckDB, or another external RDBMS. The default is an internal `ProximaRecord`/xCatalog/WAL spine with relational semantics and specialized access methods. External databases and lakehouse tables are connectors, compatibility surfaces, control-plane options, analytical projections, or explicit external-authority modes only when an ADR/xCatalog entry documents ownership, snapshot/isolation, RLS, type mapping, write/refresh, repair source, and latency trade-offs.
11h.1. **Storage And Format Fungibility:** Physical storage and external formats are flexible; semantic authority is not. LSM, row/record, PAX, columnar, Arrow, Parquet, Iceberg, Delta, Hudi, graph topology, vector, and observability formats are access methods, projections, or explicitly cataloged external authority modes. Do not let format-specific paths define their own record envelope, scalar type system, policy/RLS model, WAL/recovery semantics, or hidden catalog. External tables may be authoritative only when xCatalog records ownership, snapshot/isolation semantics, `ProximaType` mapping, policy boundary, write/refresh mode, and repair source.
11h.2. **Optional Relational Rigor:** Relational integrity is optional by workload but real when enabled. Primary keys, unique indexes, secondary indexes, foreign keys, check constraints, materialized views, transaction/isolation profiles, and stricter schema evolution are cataloged table capabilities enforced below REST/gRPC/Arrow Flight/PostgreSQL wire handlers and recovered through WAL/log/manifest.
11h.3. **Competitive Gap Alignment:** Close platform gaps through shared algebra/catalog/storage capabilities: predicate-aware vector search, hybrid BM25+dense/sparse vector+graph+document+time retrieval, reranking, Arrow-native vectorized OLAP, xCatalog projection/freshness metadata, observability/SIEM records and projections, branchable AI/data state, and open table interoperability. Do not implement these as isolated product-specific durable engines.
11i. **Modality Crates Are Not Durable Truth:** `proximadb-document`, `proximadb-graph`, `proximadb-vector`, and `proximadb-observability` own facade semantics, operators, query behavior, and projection/access-method families. They must depend on shared record/catalog/log/policy contracts and must not create hidden durable record envelopes, WALs, RLS rules, or compaction models. New durable semantics require an accepted ADR proving canonical records plus projections are insufficient.
12. **Do Not Re-Decide The Surface Every Turn:** Keep the stable layer map intact: `Foundation -> Contracts -> Modality Runtime -> Cross-Model Query Runtime -> Platform Runtime -> Apps/Bindings`.
13. **No Crate Proliferation Without Payoff:** Only add or split crates when the move cuts a heavy rebuild dependency edge, removes concrete root/runtime coupling, or creates a truly reusable contract for heavier layers.
14. **Keep Compatibility Facades Thin:** Root re-export modules are transitional compatibility seams, not a place to add new long-term behavior.
15. **Cover The Full System In Boundary Decisions:** Apply the same architectural discipline to modalities, runtime helpers, utilities, security, networking, distributed/cluster/consensus infrastructure, catalog/control-plane, storage-common, embedded/runtime composition, and external integrations.

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

## Open Format Interoperability

ProximaDB serves an **Iceberg REST Catalog v1** at `/iceberg/v1` so Spark, Trino, DuckDB, Flink, and PyIceberg can connect without custom connectors.  OLTP backends (PostgreSQL/Neon, MariaDB, SQLite) handle small-collection metadata; lakehouse catalogs handle large ones.

For the full canonical column mapping, ProximaValue→Arrow type table, engine quick-start examples, and OLTP DDL schema, read:
- `docs/12-design/OPEN_FORMAT_CATALOG_2026_05_17.adoc`
- `docs/12-design/adr/ADR-007-iceberg-rest-catalog-server.adoc`
- `docs/12-design/adr/ADR-008-oltp-catalog-backends.adoc`

### Iceberg REST Quick-Start

**Connect from Python (PyIceberg):**
```python
from pyiceberg.catalog.rest import RestCatalog
cat = RestCatalog("proximadb", **{"uri": "http://localhost:5678/iceberg/v1"})
namespaces = cat.list_namespaces()
tables = cat.list_tables(("default",))
tbl = cat.load_table(("default", "my_collection"))
df = tbl.scan().to_arrow()         # returns Arrow Table with ProximaRecord schema
```

**Connect from DuckDB:**
```sql
INSTALL iceberg; LOAD iceberg;
-- Use the manifest URL from the load-table response
```

**Connect from Spark:**
```
spark.sql.catalog.proximadb=org.apache.iceberg.spark.SparkCatalog
spark.sql.catalog.proximadb.type=rest
spark.sql.catalog.proximadb.uri=http://localhost:5678/iceberg/v1
```

### ProximaRecord Iceberg Schema (summary)

Canonical columns (full mapping in `docs/12-design/OPEN_FORMAT_CATALOG_2026_05_17.adoc`):
`id`, `tenant_id`, `created_at`, `updated_at`, `valid_from`, `valid_to`, `actor`, `origin`, `props` (map\<string,binary\>), `labels` (list\<string\>), `embedding_{model_id}` (list\<float\>), `edge_source_id`, `edge_target_id`, `edge_type`, `edge_weight`.

Table properties carry HNSW index metadata: `proximadb.index.{col}.type`, `.dim`, `.ef_construction`, `proximadb.flight.endpoint`.

Use `ProximaValue` (not legacy `SqlValue`) for all typed integrations.  Full Arrow/Parquet type mapping in the design doc above.

## 🔍 Quick Reference
- **Default Port:** 5678 (Unified), 5679 (gRPC), 5433 (PostgreSQL wire), 5680 (Arrow Flight).
- **Default Data Path:** `/tmp/proximadb/`.
- **Health Check:** `curl http://localhost:5678/health`.
- **Log Levels:** `RUST_LOG=proximadb=debug`.
- **Iceberg Catalog URI:** `http://localhost:5678/iceberg/v1`.
