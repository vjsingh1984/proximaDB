# Repository Guidelines

## Project Structure & Module Organization
- Rust product code lives under `src/`; the active domains now span `storage/`, `query/`, `graph/`, `network/`, `observability/`, `security/`, `embedded/`, `catalog/`, `cluster/`, `cdc/`, `compute/`, `services/`, and `api_handlers/`.
- Executables live in `src/bin/` and currently include the main server, migration tooling, and benchmark/data-generation binaries.
- Test coverage is split across `tests/`, `tests/integration/`, `tests/rust/`, `tests/unit/`, and `tests/tdd/`; shared fixtures/helpers live in `tests/common/` and `tests/helpers/`.
- Client surfaces now include `clients/python/`, `clients/rust/`, `clients/go/`, and embedded bindings under `clients/python-embedded/`, `clients/nodejs-embedded/`, `clients/java-embedded/`, and `clients/go-embedded/`.
- UI code lives in `ui/`; protobuf contracts live in `proto/`; deployment and packaging assets live in `deploy/`, `deployment/`, `helm/`, `k8s/`, and `packaging/`; primary docs live in `docs/`, `SUPPORTED_SURFACE.md`, and `docs/SUPPORTED_SURFACE.adoc`.

## Build, Test, and Development Commands
- `make build`, `make build-release`, `make build-server`, `make server-start`, and `make server-start-release` are the main local Rust entry points.
- `make test`, `make test-rust`, `make test-python`, `make test-integration`, `make benchmark`, and `make check` are the canonical aggregate validation targets.
- For targeted Rust work, prefer `cargo test --lib`, `cargo test --test <name>`, or `cargo test --features test-quick|test-standard|test-full`.
- When touching gated code, build or test with the relevant features explicitly: `experimental-engines`, `distributed-graph`, `tiered-graph`, `datafusion-integration`, `llm-joins`, `experimental-cdc-connectors`, `python`, `java`, `nodejs`, `c_ffi`, `aws`, `azure`, or `gcp`.
- Python SDK tests: `cd clients/python && PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python PYTHONPATH=$PWD/src python -m pytest`.
- Python embedded build/test: `cd clients/python-embedded && maturin develop --features python`, then `pytest`.
- UI workflow: `cd ui && npm start`, `npm test`, or `npm run build`.
- If `proto/` changes, regenerate Python gRPC stubs with `cd clients/python && make gen-proto`.

## Coding Style & Naming Conventions
- Rust uses edition 2024 and `cargo fmt` defaults. Keep modules focused, prefer explicit public types, and reuse shared abstractions in `core/`, `schema/`, or service-layer modules instead of introducing near-duplicate structs.
- Extend existing traits, services, proto contracts, caches, and query/graph orchestration layers before adding new infrastructure. Prefer refactoring a directionally aligned capability into the current abstraction over creating a parallel code path for the same concept.
- Treat duplicated concepts as design debt. If a new requirement overlaps an existing engine, router, planner, handler, or metadata path, converge the behavior into the canonical implementation rather than layering patchwork adapters on top.
- Multi-model design work must follow `roadmap/MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc` as the authoritative specification for the record envelope (`ProximaRecord`), the type system (`ProximaType`/`ProximaValue`), the three-layer storage model, the logical algebra, and engine-level row-level security. The ADRs in §12 of that spec are sticky decisions — do not relitigate them in isolated turns. New code that touches records, types, indexes, query lowering, storage, security, or distributed architecture MUST cite the relevant spec section and ADR ids in PRs.
- For new foundation, modality, storage, query, catalog, SDK, embedded, REST, gRPC, Arrow Flight, pgwire, and SQL-lowering contracts, use `ProximaRecord` plus `ProximaType`/`ProximaValue` because they preserve the rich datatype surface. Legacy v1 `VectorRecord`, `SqlValue`, `SqlObject`, and vector metadata maps are deprecated migration artifacts, not acceptable target contracts for new public or internal APIs.
- Protocol surfaces are adapters, not authorities. SQL/pgwire, REST, gRPC, Arrow Flight, Mongo-like document APIs, Gremlin/Cypher/PGQ graph APIs, Neptune/Titan-style compatibility, and SDK/embedded helpers must lower immediately into `ProximaValue`, `ProximaRecord`, xCatalog schema/variation metadata, or the shared logical plan. Do not let a protocol-specific request/response shape define storage semantics, type semantics, RLS, WAL/recovery, or catalog ownership.
- Schema modes must be cataloged rather than inferred in handlers: strict relational tables reject unknown fields; flexible document/graph/entity collections register structural variations; schema-on-write validates and records new variations; schema-on-read remains a query/projection behavior. Insert/upsert paths across REST, gRPC, SQL, Arrow, and embedded mode must share the same catalog/type validation and coercion rules before WAL/storage.
- Schema/API surface convergence must follow `docs/12-design/adr/ADR-009-schema-modes-and-proximarecord-surfaces.adoc`: schema-on-read external files/tables, schema-on-write OLTP loads, REST/gRPC, Arrow Flight, pgwire/SQL, SDKs, and embedded mode are all surfaces over xCatalog plus `ProximaRecord`; breaking changes are acceptable when they remove vector-only API authority.
- Relational/document/graph storage convergence work must follow `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc` as an architectural mandate. `DocumentService`, graph services, Cypher/PGQ surfaces, and document APIs are facades over canonical `ProximaRecord`/relational storage plus rebuildable projections; they must not grow independent durable semantics, separate WAL/recovery rules, or separate record envelopes. JSON path indexes, full-text indexes, adjacency tables, CSR/COO topology, and HNSW structures are adaptive projections/access methods unless an accepted ADR proves a separate durable engine is required.
- Before making or reviewing changes in records, storage, document, graph, vector, observability, query lowering, indexing, catalog, WAL/recovery, or workspace boundaries, read `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc` into the current session context and keep the mandate available for the turn. Do not rely on stale recall of the architecture decision.
- The accepted durability trajectory is stacked durability with adaptive projections, not a choice between one generic row store and many isolated durable engines. Durable authority stays in `ProximaRecord`/`ProximaType`, xCatalog, WAL/log/manifest, tenant/RLS, version/time, provenance, and retention policy. Physical specialization is expected through LSM/PAX/columnar layouts, adjacency tables, CSR/COO, graph lake projections, ANN fragments, time-series compression blocks, trace indexes, and event streams, but these structures must be cataloged, rebuildable, benchmarked, and tied to explicit freshness/rebuild semantics.
- Do not interpret stacked durability as a mandate to put ProximaDB's hot durable core inside PostgreSQL, DuckDB, or another external relational database. The accepted stack is an internal `ProximaRecord`/xCatalog/WAL spine with relational rigor and specialized access methods. External RDBMSs and lakehouse tables are connectors, compatibility surfaces, control-plane options, analytical projections, or explicit external-authority modes only when an ADR/xCatalog entry documents ownership, snapshot/isolation, RLS, type mapping, write/refresh, repair source, and latency trade-offs.
- Physical storage and external formats are fungible; semantic authority is not. LSM, row/record, PAX, columnar, Arrow, Parquet, Iceberg, Delta, Hudi, graph topology, vector, and observability formats are access methods, projections, or explicitly cataloged external authority modes. Do not let a format-specific path define its own record envelope, scalar type system, RLS/policy model, WAL/recovery semantics, or hidden catalog. External tables may be authoritative only when xCatalog records ownership, snapshot/isolation semantics, `ProximaType` mapping, policy boundary, write/refresh mode, and repair source.
- Relational rigor is optional by workload but real when enabled. Primary keys, unique indexes, secondary indexes, foreign keys, check constraints, materialized views, transaction/isolation profiles, and stricter schema evolution must be cataloged table capabilities enforced below REST/gRPC/Arrow Flight/PostgreSQL wire handlers and recovered through WAL/log/manifest.
- Competitive platform gaps to close should remain aligned with the anchor: predicate-aware vector search, hybrid BM25+dense/sparse vector+graph+document+time retrieval, reranking, Arrow-native vectorized OLAP, xCatalog projection/freshness metadata, observability/SIEM records and projections, branchable AI/data state, and open table interoperability. Add these as shared algebra/catalog/storage capabilities, not isolated product-specific engines.
- Modality crates own facade semantics, query behavior, and projection/access-method families; they do not own hidden durable truth. `proximadb-document`, `proximadb-graph`, `proximadb-vector`, and `proximadb-observability` must depend on shared record/catalog/log/policy contracts rather than inventing separate durable record envelopes, WALs, RLS rules, or compaction models. New durable semantics require an accepted ADR that explains why canonical records plus projections are insufficient.
- Data and AI platform design work must also follow `docs/12-design/DATA_AI_PLATFORM_ARCHITECTURE_ANCHOR_2026_05_12.adoc` as the anchor for competitive moat, xCatalog, pgwire compatibility, storage/compute/streaming sidecars, branchable state, agentic runtime, MLOps/MLflow/AutoML, Ray/Flink/Kafka/Kinesis integrations, and the phasewise developer-to-enterprise roadmap. Use that anchor for product/platform shape; use the multi-modal overhaul spec for record/type/algebra/storage/security internals.
- Best-in-class engineering principles enforced via the overhaul spec: scalable (disaggregated log/storage/compute, three-tier cache, Arrow Flight transport), maintainable (one record envelope, one type system, ADR-tracked decisions), extensible (operators added once in the algebra, surface languages cheap, modality crates symmetric), and robust (engine-level RLS, atomic metadata+vector writes, subschema-checked schema evolution, predicate-aware HNSW).
- Workspace architecture work must follow `roadmap/techdebt/WORKSPACE_REFACTOR_PLAN_2026_05_07.adoc` as the current source of truth for the stable skeleton, layer boundaries, and freeze rules. Do not improvise a different crate/layout strategy in isolated turns.
- Favor skeleton-first movement over surface churn. Keep the high-level layer map stable: `Foundation -> Contracts -> Modality Runtime -> Cross-Model Query Runtime -> Platform Runtime -> Apps/Bindings`.
- Do not create new crates or move public surfaces unless the change severs a heavy rebuild dependency edge, removes concrete root/runtime coupling, or establishes a genuinely reusable contract consumed by heavier crates.
- Keep root compatibility modules stable during migration, especially `src/query/unified/ast.rs`, `src/query/unified/uql.rs`, `src/query/multimodal/plan.rs`, and similar re-export shells. Move behavior behind extracted crates instead of redesigning the facade every turn.
- Cover the whole product in boundary decisions, not just query/graph code. That includes modalities, runtime helpers, utilities, storage-common, security, networking, cluster/consensus, catalog/control-plane, embedded/runtime composition, and external integrations.
- Default server behavior assumes `sql_frontend`, `graph-first-sks`, and `unified-facade-routing`; do not widen default surface area casually.
- Keep protocol-facing behavior aligned across REST, gRPC, Arrow Flight, and PostgreSQL wire paths when a feature is meant to be multi-surface.
- Design changes should remain robust under the distributed roadmap: favor deterministic behavior, idempotent APIs, explicit ownership boundaries, composable interfaces, and observability hooks over hidden coupling or one-off shortcuts.
- Python code should stay Black/Ruff-compatible and snake_case; treat generated files under `clients/python/src/proximadb/v1/` as outputs, not hand-edited source.
- UI code is TypeScript/React 17 with Material-UI-era patterns; preserve the existing structure unless the task is an intentional UI refactor.

## Testing Guidelines
- Keep unit tests close to implementation; use `tests/*.rs` and `tests/integration/**` for engine parity, query routing, API, and recovery scenarios.
- Many integration tests bind ports or start services. If failures look timing- or port-related, rerun with `cargo test -- --test-threads=1`.
- For Rust feature work, run the narrowest relevant test first, then a broader sweep with `cargo test --features test-standard` or `make test-rust`.
- For client changes, run the affected SDK suite plus protocol-specific coverage (`rest`, `grpc`, or embedded) before broadening.
- Do not infer maturity from code presence alone. Keep examples, docs, and feature claims aligned with `SUPPORTED_SURFACE.md`, `docs/SUPPORTED_SURFACE.adoc`, and `docs/10-quality/TECHNICAL_DEBT.adoc`.

## Commit, PR, and Security Guidance
- Use concise present-tense commit subjects and keep them under 72 characters.
- PRs should describe behavior changes, feature flags touched, tests run, and any impact to public APIs, docs, or supported-surface claims.
- When a change replaces or extends existing capability, call out what was reused, what was refactored, and what duplicate path was avoided or removed.
- Avoid broad formatting churn in this tree; keep refactors scoped and update adjacent docs/examples when behavior changes.
- Treat `certs/`, `config/`, `configs/`, auth/security modules, and deployment manifests as security-sensitive. Keep secrets out of the repo and prefer env/config samples.
- Before enabling or documenting a feature by default, verify whether it is `Supported`, `Beta`, or `Experimental` in the current supported-surface documents.

---

## Open Format Catalog (Iceberg REST, OLTP Backends, xCatalog)

ProximaDB serves an **Iceberg REST Catalog v1** at `/iceberg/v1` (port 5678) — Spark, Trino, DuckDB, Flink, and PyIceberg connect without a custom connector.  OLTP catalog backends (PostgreSQL/Neon/Supabase, MariaDB, SQLite) handle small-collection metadata; lakehouse catalogs handle large ones.  All open-format paths are Layer 2–3 projections over the canonical WAL → `ProximaRecord` → storage-engine spine.

**Authoritative design docs** — read these before touching catalog, open-format, or OLTP code:

- `docs/12-design/OPEN_FORMAT_CATALOG_2026_05_17.adoc` — full architecture, ProximaRecord→Iceberg column mapping, type system, external engine quick-start, key papers
- `docs/12-design/adr/ADR-007-iceberg-rest-catalog-server.adoc` — Iceberg REST decision, endpoints, synthetic snapshot, Hadoop split mental model
- `docs/12-design/adr/ADR-008-oltp-catalog-backends.adoc` — OLTP decision, DDL schema, size-based routing, backend feature flags

Key source files: `src/catalog/iceberg_rest_service.rs`, `src/network/rest/v1/iceberg_rest_catalog.rs`, `src/catalog/oltp.rs`, `src/catalog/mod.rs` (`catalog_for_size()`), `proto/proximadb/v1/catalog.proto`.

---

## Stacked Durability Layers (ADR-010)

```
Layer 0: WAL (CanonicalOperation::RecordUpsert)      — durable journal, crash-safe
Layer 1: ProximaRecord in VIPER/NOVA/HELIX           — canonical record engines
Layer 2: PAX blocks + PaxSegment files (*.pax)       — rebuildable columnar projections ← NEW
Layer 3: Iceberg REST / Arrow Flight facades          — external protocol adapters
```

**Key invariant**: Layers 1-3 are rebuildable from Layer 0.  Never introduce a separate WAL or transaction semantics for a Layer 2/3 projection.

### PAX Block Format (`proximadb-block-format`)

Physical layout (one block):
```
[BlockHeader: 64B][RowDirectory: N×32B][ColumnStripes][ColumnMeta × N_cols: 64B each][BlockFooter: 32B]
```

**Predicate pruning hierarchy** (evaluated without reading full block data):
1. `BlockHeader.tenant_id_hash` mismatch → skip block  (engine-level RLS skip)
2. `BlockHeader.min_timestamp_ns / max_timestamp_ns` outside query range → skip block
3. `ColumnMeta.min_val / max_val` excludes predicate value → skip stripe
4. `RowEntry.valid_from_ns / valid_to_ns` outside query snapshot → skip row (MVCC)

**Canonical Column IDs (ADR-010, stable — never reuse an ID)**:

| ID | Field | Role |
|----|-------|------|
| 0 | `oid` | Record identity |
| 1 | `tenant_id` | Tenancy / RLS partition key |
| 2 | `created_at_ns` | Wall-clock insert timestamp |
| 3 | `updated_at_ns` | Last mutation timestamp |
| 4 | `valid_from_ns` | Bi-temporal valid-time start |
| 5 | `valid_to_ns` | Bi-temporal valid-time end |
| 6 | `actor` | Provenance: who caused the write |
| 7 | `origin` | Provenance: system/pipeline origin |
| 8 | `props` | msgpack-encoded property map |
| 9 | `labels` | string list (tags) |
| 10 | `edge_src` | Graph edge source OID |
| 11 | `edge_tgt` | Graph edge target OID |
| 12 | `edge_type` | Graph edge type string |
| 13 | `edge_weight` | Graph edge weight (f32) |
| 20 | `embed_base` | Base of embedding columns (model 0) |
| 100+ | User-defined | Application-specific columns |

**Block modes**: `Pax` (default — row dir + column stripes), `Oltp` (row dir only), `Olap` (column stripes only).
Use `SelectionContext::for_pax_stripe()` / `::for_olap_stripe()` / `::for_oltp_row()` to tell the codec the access pattern.

**PaxSegment layer** (`proximadb-storage-common::pax_block`):
- `PaxSegmentWriter` — file-backed multi-block writer; auto-flushes at threshold; returns `SegmentMeta` with `BlockStats`
- `PaxSegmentScanner` — reads segment index from file tail, yields `PaxBlockReader` per block passing tenant/time predicates
- Segment files: `*.pax`, terminated with `b"PAXSEG01"` magic + CRC-verified index

---

## Protocol Surfaces

| Protocol | Default Port | Use |
|----------|-------------|-----|
| REST/gRPC unified | 5678 | Primary API, Iceberg REST (`/iceberg/v1`), health |
| gRPC multi-port | 5679 | Dedicated gRPC when `api.unified_mode = false` |
| PostgreSQL wire | 5433 | pgvector-compatible SQL queries (`<->` operator) |
| Arrow Flight | 5680 | High-throughput record streaming, Arrow Flight SQL |

### Querying ProximaDB

**Arrow Flight SQL** (recommended for agents, high-throughput):
```python
from pyarrow import flight
client = flight.connect("grpc://localhost:5680")
result = client.execute("SELECT * FROM my_collection WHERE tenant_id = 'acme' LIMIT 100")
table = result.read_all()
```

**AQL** (ProximaDB native query language):
```
SEARCH my_collection
WHERE labels CONTAINS 'product'
VECTOR SEARCH embedding NEAR [0.1, 0.2, ...] TOP 10
RETURN id, props, _score
```

**Natural Language** (via `/api/v1/query/nl`):
```json
{ "query": "Find products similar to the blue running shoe", "collection": "my_collection" }
```

---

## xCatalog as Single Control Plane

All catalog operations (namespace, schema, stats, partitions, snapshots) go through `CatalogManager` in `src/catalog/mod.rs`.  The `Catalog` trait (`crates/control/proximadb-catalog/src/lib.rs`) is the canonical interface; backends implement it without root dep.

**Size-based routing** (`catalog_for_size(bytes)`):
- `< 1 GB` → `OltpCatalog` (PostgreSQL/Neon/Supabase, MariaDB, SQLite — fast ACID metadata)
- `≥ 1 GB` → lakehouse catalog (Iceberg REST, Delta, native)

**Feature flags** (build with `--features oltp-catalog-postgres` etc.):
```
oltp-catalog-postgres  — Neon / Supabase / CockroachDB
oltp-catalog-mysql     — MariaDB / TiDB / PlanetScale
oltp-catalog-sqlite    — SQLite (embedded/dev, default DSN: sqlite::memory:)
```

---

## Sticky ADR Reference

| ADR | Title | Decision |
|-----|-------|----------|
| ADR-007 | Iceberg REST Catalog Server | Mount at `/iceberg/v1`; synthetic snapshots; Arrow Flight tickets in write-credentials |
| ADR-008 | OLTP Catalog Backends | Metadata-only (never record data); sqlx; size-threshold routing at 1 GB |
| ADR-009 | Schema Modes & ProximaRecord Surfaces | All protocol surfaces are xCatalog + ProximaRecord; breaking vector-only API authority is acceptable |
| ADR-010 | PAX Block Format (Layer 2) | Hybrid row dir + column stripes; 64B BlockHeader; stable canonical column IDs; no tokio in block crate |
