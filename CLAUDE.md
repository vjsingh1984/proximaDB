# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Quick Reference

| Resource | Value |
|----------|-------|
| **Default Ports** | 5678 (unified REST/gRPC), 5679 (gRPC multi-port), 5433 (PostgreSQL wire), 5680 (Arrow Flight) |
| **Default Data** | `/tmp/proximadb/` |
| **Config File** | `config/config.toml` |
| **Health Check** | `curl http://localhost:5678/health` |
| **WAL Manifest** | `/tmp/proximadb/manifest/manifest_*.jsonl` |
| **Main Branch** | `main` |

## Build and Development

**Rust stable channel** (see `rust-toolchain.toml`) | Edition 2024 | Cargo workspace: root + `clients/rust`

> **Note**: `rust-toolchain.toml` sets `targets = ["x86_64-unknown-linux-gnu"]`. On macOS, if you hit target errors, run `rustup target add aarch64-apple-darwin`.

```bash
# Build
cargo build                      # Debug (opt-level=0, fast compilation)
cargo build --release            # Release (opt-level=3, full LTO)
cargo build --profile release-server  # Optimized server build
cargo check --all-targets        # Fast syntax check (no codegen)

# Run server
cargo run --bin proximadb-server                  # Debug mode
cargo run --release --bin proximadb-server         # Release mode

# Test (dev and test profiles match for 100% artifact reuse)
cargo test                       # All tests (shares artifacts with cargo build)
cargo test --lib                 # Unit tests only (in src/)
cargo test --test integration    # tests/rust/mod.rs
cargo test --test graph_integration_test  # Graph tests
cargo test test_name             # Specific test by name
cargo test -- --test-threads=1   # Sequential (for port-binding tests)

# Quality
cargo fmt                        # Format code
cargo clippy -- -D warnings      # Lint (warnings are errors locally)

# Full check (format, lint, test)
cargo fmt && cargo clippy -- -D warnings && cargo test

# Run specific test with debug output
RUST_LOG=debug cargo test test_name -- --nocapture --test-threads=1

# Python SDK
cd clients/python && pip install -e .
PYTHONPATH=clients/python/src pytest clients/python/tests/ -v
PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python PYTHONPATH=clients/python/src pytest clients/python/tests/ -v  # if proto issues

# Rust SDK
cd clients/rust && cargo build --features client

# Web UI Dashboard
cd ui && npm install && npm start

# Benchmarks
cargo bench                      # Criterion benchmarks
cargo run --bin proximadb-bench  # Consolidated benchmark tool

# Other binaries
cargo run --bin proximadb-migrate  # Schema migration tool
cargo run --bin ann-benchmarks     # ANN benchmark suite
```

**Profile Strategy**: dev = test (opt-level=0) for 100% artifact reuse; dependencies (arrow/parquet) at opt-level=2 in dev; `release-server` profile for optimized server builds. Library crate produces both `rlib` and `cdylib` (cdylib for PyO3 embedded bindings). `build.rs` watches `proto/` and triggers proto regeneration via prost-build on changes.

### Test Organization

| Location | Type | Purpose | Command |
|----------|------|---------|---------|
| `src/**/*.rs` | **Inline Unit Tests** (`#[cfg(test)]`) | Internal functions, private APIs | `cargo test --lib` |
| `tests/unit/` | **Legacy Unit Tests** (being migrated) | Tests being inlined into source | `cargo test --lib` |
| `tests/integration/` | **Integration Tests** | Cross-module integration | `cargo test --test integration` |
| `tests/*_integration_test.rs` | **Engine Integration** | Storage engine integration tests | `cargo test --test <name>_integration_test` |
| `tests/graph_integration_test.rs` | **Graph Database Tests** | Graph functionality | `cargo test --test graph_integration_test` |
| `tests/graph_rag_integration_test.rs` | **Graph RAG Tests** | Graph RAG integration | `cargo test --test graph_rag_integration_test` |
| `tests/wal_*.rs` | **WAL Persistence** | Write-ahead log persistence | `cargo test --test wal_path_correctness_test` |
| `clients/python/tests/` | **Python SDK Tests** | Python client library | `PYTHONPATH=clients/python/src pytest clients/python/tests/ -v` |
| `benches/` | **Benchmarks** | Performance benchmarks | `cargo bench` |

**Test practices:**
- Use `--test-threads=1` for tests that bind ports (network tests) to avoid race conditions
- `RUST_LOG=debug` for debugging: `RUST_LOG=debug cargo test test_name -- --nocapture`
- Clean test data between runs: `rm -rf /tmp/proximadb*`
- CI feature flags: `test-quick` (unit only), `test-standard` (unit + integration), `test-full` (all categories)

### Common Operations

```bash
# Kill all ProximaDB servers
pkill -f proximadb-server

# Reset test data
rm -rf /tmp/proximadb*

# Clean rebuild
cargo clean && cargo build

# Debug logging (component-specific)
RUST_LOG=proximadb::storage=trace cargo run --bin proximadb-server
RUST_LOG=proximadb::graph=debug cargo test --test graph_integration_test

# WAL diagnostics
cat /tmp/proximadb/manifest/manifest_*.jsonl | jq .

# Code coverage
cargo llvm-cov --lib --html --output-dir coverage
```

## Development Philosophy

- **Concrete Over Speculative**: Working implementations over TODO comments
- **No Mocking**: Real implementations, test against real data
- **Practical Over Perfect**: Simplest working version first
- **Evidence-Based**: Decisions backed by benchmarks and data
- **Test-Driven Development**: TDD methodology with pre-commit hooks (`make install-tdd-hooks`)

**Technical Debt**: Track in `docs/10-quality/TECHNICAL_DEBT.adoc` with numbered TD-XXX items. Prioritize by impact/effort. Regular reviews during sprint planning.

### Reuse-First Architecture Rules

- Prefer extending the existing service, engine, router, cache, proto, and handler layers before introducing new infrastructure. If a need is directionally aligned with a current capability, refactor and converge it into that capability instead of creating a parallel implementation.
- Treat duplicate abstractions as a regression. New structs, traits, endpoints, planners, or storage paths should only be introduced when the existing surface cannot be cleanly evolved.
- Keep changes compatible with the distributed roadmap: explicit ownership, deterministic behavior, idempotent operations, well-bounded interfaces, and first-class observability are required. Hidden global coupling and one-off code paths are not acceptable tradeoffs for short-term speed.
- Prefer composition over proliferation. Reuse canonical proto types, `core`/`services` abstractions, shared query/graph orchestration, and metadata infrastructure so behavior stays aligned across REST, gRPC, Arrow Flight, PostgreSQL wire, and future distributed execution.

### Multi-Model Overhaul Mandate (Authoritative)

- Treat [roadmap/MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc](roadmap/MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc) as the authoritative specification for record format, type system, modality boundaries, query algebra, security placement, and cross-model design. It supersedes prior architecture review documents (`docs/MULTI_MODEL_ARCHITECTURE_REVIEW.md`, `docs/10-quality/MULTIMODEL_ARCHITECTURE_ANALYSIS.adoc`) for go-forward design decisions.
- The ADRs in §12 of the spec are sticky decisions; do NOT relitigate them in isolated turns. If a turn proposes a deviation, surface the ADR id explicitly and justify the change against the convergent research in §2 and the gap analysis in §1.2.
- Sticky design pillars ALL new code must respect:
  - **One record envelope** (`ProximaRecord`, spec §3): every modality projects onto identity, variation, tenancy, time, provenance, props, refs, edge, embeddings, sequence, labels, extensions. Do not introduce parallel record shapes.
  - **One scalar type system** (`ProximaType`/`ProximaValue`, spec §4): catalog, wire, storage, and pgwire must agree on Decimal, TimestampTz, Uuid, Json, Vector. No new oneof variants in legacy `SqlValue`.
  - **Canonical model first across every surface**: new foundation, modality, storage, query, catalog, SDK, embedded, REST, gRPC, Arrow Flight, pgwire, and SQL-lowering contracts use `ProximaRecord` plus `ProximaType`/`ProximaValue`. Legacy v1 `VectorRecord`, `SqlValue`, `SqlObject`, and vector metadata maps are deprecated migration artifacts, not target public or internal API contracts.
  - **Protocol adapters are not semantic authorities**: SQL/pgwire, REST, gRPC, Arrow Flight, Mongo-like document APIs, Gremlin/Cypher/PGQ graph APIs, Neptune/Titan-style compatibility, SDKs, and embedded helpers must lower into `ProximaValue`, `ProximaRecord`, xCatalog schema/variation metadata, or the shared logical plan. Do not let protocol request/response shapes define storage, type, RLS, WAL/recovery, or catalog semantics.
  - **Cataloged schema modes**: strict relational tables, flexible document/graph/entity collections, schema-on-write variation registration, and schema-on-read projection behavior must be xCatalog/table capabilities. Insert/upsert paths across SQL, REST, gRPC, Arrow, and embedded mode share the same type validation and coercion before WAL/storage.
  - **ADR-009 schema/API convergence**: schema-on-read external files/tables, schema-on-write OLTP loads, REST/gRPC, Arrow Flight, pgwire/SQL, SDKs, and embedded mode are surfaces over xCatalog plus `ProximaRecord`; breaking changes are acceptable when they remove vector-only API authority.
  - **Three-layer storage** (spec §5): unified record (PAX) + topology (CSR/COO) + vector index (HNSW relationship table) behind one buffer pool.
  - **One logical algebra** (spec §7): SQL/Cypher/AQL/UQL/PromQL/LogQL all compile to `Filter, Project, Sort, Limit, Aggregate, Union, Join, HybridTraverse, PatternMatch, CrossModelJoin, VectorTopK, ModulationOp, MatrixOp, SemanticJoin, ModelConvert`.
  - **Engine-level RLS** (spec §8): `tenant_id`/`permitted_principals` are record fields, pushed into scan iterators. No application-layer-only tenant filtering.
  - **Predicate-aware HNSW** (spec §6.1): γ-expanded ACORN construction + NaviX adaptive-local search + per-modality shards (HMGI) + PAX co-location of filter columns.
- New work that touches records, types, indexes, query, storage, security, or distributed architecture MUST cite the relevant spec section and ADR ids in the PR description.
- The Phase A-G roadmap (spec §11) is the sequencing contract. New TD entries and roadmap updates must reference Phase A/B/C/D/E/F/G/H or explain why the work falls outside.
- Best-in-class engineering principles enforced via this mandate: scalable (disaggregated runtime per Bacchus, three-tier cache), maintainable (one envelope, one type system, ADR-tracked decisions), extensible (operators added once in the algebra, surface languages cheap, modality crates symmetric), and robust (engine-level RLS, atomic metadata+vector writes, subschema-checked schema evolution).

### Data and AI Platform Anchor

- Treat [docs/12-design/DATA_AI_PLATFORM_ARCHITECTURE_ANCHOR_2026_05_12.adoc](docs/12-design/DATA_AI_PLATFORM_ARCHITECTURE_ANCHOR_2026_05_12.adoc) as the product/platform anchor for competitive moat, xCatalog, pgwire compatibility, commodity compute routing, streaming integration, branchable state, agentic runtime, MLOps/MLflow/AutoML, Ray/Flink/Kafka/Kinesis support, and the phasewise developer-to-enterprise roadmap.
- Use the platform anchor to decide product shape and sequencing. Use the multi-model overhaul spec to decide internal record/type/algebra/storage/security shape. If they appear to conflict, preserve the overhaul spec for internals and update the anchor rather than adding a third direction.
- Platform changes involving pgwire, catalog, DataFusion/Spark/Trino/Ray/Flink adapters, Kafka/Kinesis/MQ connectors, MLflow/MLOps, branch/restore, or agent state must map to one of the anchor phases and name the affected plane in PRs.

### Relational/Document/Graph Convergence Mandate

- Treat [docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc](docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc) as the required design for document and graph storage evolution.
- Before making or reviewing changes in records, storage, document, graph, vector, observability, query lowering, indexing, catalog, WAL/recovery, or workspace boundaries, read the convergence doc into the current session context and keep the mandate available for the turn. Do not rely on stale recall of the architecture decision.
- `DocumentService`, graph services, Cypher/PGQ surfaces, document APIs, and SDK modality helpers are facades over canonical `ProximaRecord`/relational storage. They must not become independent durable systems with separate record envelopes, transaction semantics, WAL/recovery paths, tenant/RLS enforcement, or compaction rules.
- Durable documents, graph nodes, graph edges, and relational rows converge on `ProximaRecord` plus `ProximaType`/`ProximaValue`. JSON path indexes, array indexes, full-text indexes, adjacency tables, CSR/COO topology, columnar variation projections, and HNSW indexes are rebuildable projections/access methods over canonical records.
- New modality contracts must not fall back to legacy v1 `SqlValue`/`SqlObject` just because existing services still expose them. Convert legacy protocol/service shapes at the edge; keep the modality foundation on `ProximaRecord`/`ProximaValue` and v2-compatible rich datatypes.
- The accepted trajectory is **stacked durability with adaptive projections**. Durable authority lives in `ProximaRecord`/`ProximaType`, xCatalog, WAL/log/manifest, tenant/RLS, version/time, provenance, and retention policy. Physical specialization is expected through LSM/PAX/columnar layouts, adjacency tables, CSR/COO, GraphAr-style lake projections, ANN fragments, time-series compression blocks, trace indexes, and event streams, but these are cataloged physical structures with freshness, rebuild, and benchmark evidence.
- Do not interpret stacked durability as delegating ProximaDB's hot durable core to PostgreSQL, DuckDB, or another external RDBMS. The default is an internal `ProximaRecord`/xCatalog/WAL spine with relational semantics and specialized access methods. External databases and lakehouse tables are connectors, compatibility surfaces, control-plane options, analytical projections, or explicit external-authority modes only when an ADR/xCatalog entry documents ownership, snapshot/isolation, RLS, type mapping, write/refresh, repair source, and latency trade-offs.
- Physical storage and external formats are fungible; semantic authority is not. LSM, row/record, PAX, columnar, Arrow, Parquet, Iceberg, Delta, Hudi, graph topology, vector, and observability formats are access methods, projections, or explicitly cataloged external authority modes. Do not let a format-specific path define its own record envelope, scalar type system, policy/RLS model, WAL/recovery semantics, or hidden catalog.
- Relational rigor is optional by workload but real when enabled. Primary keys, unique indexes, secondary indexes, foreign keys, check constraints, materialized views, transaction/isolation profiles, and stricter schema evolution must be cataloged table capabilities enforced below REST/gRPC/Arrow Flight/PostgreSQL wire handlers and recovered through WAL/log/manifest.
- Competitive platform gaps to close should remain aligned with the anchor: predicate-aware vector search, hybrid BM25+dense/sparse vector+graph+document+time retrieval, reranking, Arrow-native vectorized OLAP, xCatalog projection/freshness metadata, observability/SIEM records and projections, branchable AI/data state, and open table interoperability. Add these as shared algebra/catalog/storage capabilities, not isolated product-specific engines.
- Do not frame future work as "generic row store vs separate durable engines." The research-backed tradeoff is one durable semantic spine plus specialized physical layouts and projections. New durable semantics require an accepted ADR proving canonical records plus projections cannot meet correctness or performance needs.
- CSR is an adaptive graph topology projection for read-heavy traversal and graph algorithms, not the durable graph source of truth. Write-heavy graph workloads should use relational adjacency tables/indexes first; CSR materialization requires workload evidence and freshness/invalidation rules.
- CEDAR, ORION, TITAN, and similar modality-specific engines must either be projection/access-method implementations over canonical records or temporary compatibility adapters with a retirement plan. Creating or extending a separate durable document/graph engine requires an accepted ADR and benchmark/correctness evidence that canonical records plus projections cannot satisfy the requirement.
- Modality crates own facade semantics, operators, and projection/access-method families. `proximadb-document`, `proximadb-graph`, `proximadb-vector`, and `proximadb-observability` must depend on shared record/catalog/log/policy contracts and must not own hidden durable truth.

### Workspace Skeleton Mandate

- Treat [roadmap/techdebt/WORKSPACE_REFACTOR_PLAN_2026_05_07.adoc](roadmap/techdebt/WORKSPACE_REFACTOR_PLAN_2026_05_07.adoc) as the active source of truth for workspace shape, boundary validation, and migration trajectory.
- The stable target skeleton is: `Foundation` → `Contracts` → `Modality Runtime` → `Cross-Model Query Runtime` → `Platform Runtime` → `Apps/Bindings`.
- Optimize for cleaner boundaries, not more crates. New crates are justified only if they cut a heavy rebuild edge, remove concrete root/runtime coupling, or establish a stable reusable contract that multiple heavier crates will consume.
- Keep compatibility shims thin and stable. Avoid adding fresh behavior to root re-export modules when the behavior can live behind an extracted crate.
- Make boundary decisions across the whole product, including query, graph, vector, document, observability, storage-common, security, networking, distributed/cluster/consensus, catalog/control-plane, runtime helpers, embedded/runtime composition, and external integrations.

### Workspace Layering Rules

**Golden Rule**: Downward dependencies only. Higher layers may depend on lower layers, but lower layers must NEVER depend on higher layers.

**Layer Hierarchy** (bottom to top):
```
┌─────────────────────────────────────────┐
│  Apps/Bindings (CLI, SDKs, embedded)   │ ← Highest
├─────────────────────────────────────────┤
│  Platform Runtime (server, networking)  │
├─────────────────────────────────────────┤
│  Cross-Model Query Runtime (SQL, graph) │
├─────────────────────────────────────────┤
│  Modality Runtime (vector, graph, doc)  │
├─────────────────────────────────────────┤
│  Contracts (traits, proto definitions)  │
├─────────────────────────────────────────┤
│  Foundation (types, error, utils)       │ ← Lowest
└─────────────────────────────────────────┘
```

**Forbidden Patterns**:
- ❌ Foundation types importing from platform runtime (e.g., `use crate::network::*`)
- ❌ Contracts depending on modality runtime implementations
- ❌ Storage engines importing from query layer
- ❌ Circular dependencies between any layers

**Correct Patterns**:
- ✅ Platform runtime imports from contracts and foundation
- ✅ Modality runtime imports from contracts and foundation
- ✅ Cross-model query runtime imports from modality runtime and contracts
- ✅ Apps/bindings import from any layer (as needed)

**Layering Violation Examples**:
```rust
// ❌ FORBIDDEN: Foundation importing from network
use crate::network::rest::server;  // Wrong!

// ❌ FORBIDDEN: Contracts importing from storage
use crate::storage::engines::viper;  // Wrong!

// ✅ CORRECT: Network importing from contracts
use crate::proto::proximadb_v1;  // Correct

// ✅ CORRECT: Storage importing from foundation
use crate::core::types::ProximaType;  // Correct
```

**Code Review Checklist**:
- [ ] No imports from higher layers in lower layer files
- [ ] No circular dependencies between modules
- [ ] Foundation types remain generic and reusable
- [ ] No implementation details in contracts layer
- [ ] No platform-specific code in modality runtime

**Verification**: Run `scripts/check-layering.sh` to detect violations (Phase 5.2).

### Document / Graph Durability Checklist (required for any PR touching document, graph, WAL, or recovery code)

Before merging any change that writes document or graph data:
- [ ] Does the change write through `CanonicalOperation::RecordUpsert` / `RecordDelete` in the canonical WAL?
- [ ] If a new physical projection is added, does a `ProjectionDirective` variant exist so recovery can rebuild it?
- [ ] If a legacy `DocumentOperation::InsertDocument` or `GraphOperation::CreateNode` path is touched, is it being migrated toward canonical WAL, not extended?
- [ ] No new independent WAL or recovery semantics introduced for document/graph state (requires accepted ADR if so).
- [ ] `docs/12-design/SUPPORTED_SURFACES.adoc` updated if projection status changed.

Reference: `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc`

### Code Quality Standards

**Enforced via clippy lints** (`src/lib.rs:22-31`):
- NO `.unwrap()`, `.expect()`, or `panic!()` in production code
- Use `Result<T, Error>` and `?` operator for error propagation
- Critical sections may use `.unwrap()` with extensive justification comments

**Error types**: `ProximaDBError` (`src/core/errors/core_error.rs`) is the main domain error. `ApiError` (`src/errors/mod.rs`) wraps it for REST/gRPC responses (converts to HTTP status codes and gRPC Status). `StorageError` lives in both `src/core/error.rs` and `src/storage/error.rs` — check which layer you're in.

**Recursion limit**: `#![recursion_limit = "1024"]` is set in `src/lib.rs` due to deeply nested Serde types.

**CI divergence**: CI uses `RUSTFLAGS: "-A warnings"` to avoid blocking on accumulated technical debt, but local development should still target `cargo clippy -- -D warnings`.

### Documentation Standards

- **AsciiDoc Only**: All docs in `.adoc` format
- **Mermaid Diagrams**: Use `[source,mermaid]` blocks with `%%{init: {"theme": "neutral"}}%%`
- **Colors**: Primary `#4a90e2`, text `#000` on fills, borders `#2e5c8a`
- **Avoid**: Casual emojis, decorative symbols

## Memory System

This codebase uses an auto-memory system at `/Users/vijaysingh/.claude/projects/-Users-vijaysingh-code-proximaDB/memory/`. Memory persists across conversations and contains user preferences, feedback, project context, and reference pointers. When the user explicitly asks to remember or forget information, update the memory system immediately.

## Architecture Overview

Unified vector database (v0.2.0) with 6 specialized storage engines, native graph, federated multi-model query engine, and AutoML.

### Core Components

- **Storage** (`src/storage/`): 6 engines (SST, HELIX, VIPER, SWIFT, NOVA, RAPTOR) implementing `UnifiedStorageEngine` trait. Engine selection via `src/storage/engines/factory.rs`. `src/storage/document/sdp.rs` — Storage Data Plane for document operations.
- **Compute** (`src/compute/`): Unified quantization (`quantization/unified.rs`), hardware-accelerated distance (L2, cosine, dot product) with runtime SIMD detection (AVX2/AVX512/NEON, scalar fallback)
- **Index** (`src/index/`): AXIS engine (HNSW, IVF, PQ, Annoy, LSH), DiskANN (Vamana graph + SSD layout), geo-spatial (geohash)
- **Network** (`src/network/`): REST + gRPC + Arrow Flight + PostgreSQL wire protocol orchestration (`multi_server.rs`). Unified port mode (default) or multi-port.
- **API Handlers** (`src/api_handlers/`): Request handling logic for unified, AI, and enterprise endpoints
- **Services** (`src/services/`): Collection management, vector operations (`operations/vectors.rs`), EventLog
- **Query** (`src/query/`): Federated multi-model engine with SQL extensions (`VECTOR_SEARCH()`, `GRAPH_QUERY()`, `DOCUMENT_QUERY()`, `LOGS()`, `METRICS()`). RL-based query planner with Thompson Sampling. Includes `src/query/aql/` (AQL), `src/query/nl/` (Natural Language), `src/query/unified/` (unified execution).
- **Graph** (`src/graph/`): Native graph with ORION (in-memory + WAL), PULSAR (distributed), QUASAR (hybrid vector+graph) engines. CSR format, Arc-based zero-copy. `src/graph/rag/` — Graph RAG (RGL) with multiple engine implementations.
- **Observability** (`src/observability/`): Logs, metrics, traces with 6 SIEM adapters
- **CDC** (`src/cdc/`): Outbound change data capture (Kafka, webhook sinks)
- **AutoML** (`src/automl/`): Automated optimization, workload prediction, tuning
- **Embedded** (`src/embedded/`): In-process database with PyO3, JNI, NAPI-RS, C FFI bindings

### Data Flow

**Write**: `network/` → `api_handlers/` → `services/operations/vectors.rs` → `storage/persistence/write_ahead_log/` → engine-specific write → `storage/persistence/filesystem/`

**Read**: `network/` → `api_handlers/` → `services/` → `core/search/` → engine-specific search → `compute/distance/` → `storage/persistence/filesystem/`

**Server lifecycle**: Hardware detection → SharedServices → Global Metadata Provider (WAL paths) → Global WAL Manifest → Storage Engine (WAL recovery) → Multi-Server. Shutdown reverses with Graph WAL flush (5s timeout) first.

### Proto-First Architecture

All API types defined in `proto/proximadb.proto` and `proto/proximadb/v1/*.proto`. Use proto types directly (zero-copy principle). Proto types have Serde compatibility for JSON/REST. Run `cargo build` after modifying protos to regenerate. Generated code at `src/proto/proximadb.v1.rs`.

### Feature Flags

**Default** (always on): `sql_frontend`, `graph-first-sks` (OrionBackedEntityStore), `unified-facade-routing`

**Optional**:
- `cloud-full`: All cloud backends (S3, Azure, GCS). Also `aws`, `azure`, `gcp` individually
- `rocksdb`: RocksDB metadata backend
- `gpu`: Metal GPU acceleration (macOS; CPU fallback on other platforms)
- `experimental-engines`: RAPTOR and SWIFT engines (disabled by default to prevent panics from unimplemented paths)
- `experimental-cdc-connectors`: Native CDC database connectors (partial; prefer Debezium for production)
- `cluster`: Distributed consensus (Raft), replication, health services
- `distributed`: Multi-node deployment support

### Configuration (`config/config.toml`)

Key sections: `[server]`, `[storage]`, `[storage.wal_config]`, `[api]`, `[monitoring]`, `[security]`, `[query.rl_planner]`, `[llm]`, `[distributed]`

Protocol modes: Unified port (default, all on 5678), PostgreSQL wire (5433, pgvector-compatible with `<->` operator), Multi-port (`api.unified_mode = false`)

Config variants: `config/minimal.toml`, `config/test-config.toml`, `config/production.toml`, `config/cloud-s3.toml`

## CI/CD

| Workflow | Trigger | Purpose |
|----------|---------|---------|
| `ci.yml` | Push/PR to main | Format, clippy, security audit, build, test (all categories), coverage, Python SDK, Docker build |
| `release.yml` | Version tags / manual | Multi-platform builds (6 targets), Python wheels, PyPI/crates.io/GitHub Releases |
| `tdd.yml` | Manual / scheduled | nextest, flaky detection (3x runs), coverage thresholds, benchmark regression |

Coverage threshold: 60% (cargo-tarpaulin). `dorny/paths-filter` runs targeted test suites based on changed components.

**Makefile targets**: `make build` / `make build-release` / `make build-server`, `make test` / `make test-rust` / `make test-python`, `make integration-full` (starts release server + Python integration), `make server-start` / `make server-start-release`, `make fmt`, `make clippy`, `make check` (fmt + clippy + test), `make test-tdd`, `make test-coverage`, `make benchmark` / `make benchmark-vector` / `make benchmark-metadata`, `make tdd-precommit`, `make test-tdd-module MODULE=core::search::hybrid`, `make test-watch` (requires cargo-watch), `make panic-policy-report`

**Docker**: Two Dockerfiles — root `Dockerfile` and `deploy/docker/Dockerfile` (both multi-stage: rust:1.88-slim builder, python:3.11-slim runtime). Compose at `deploy/docker/docker-compose.yml` includes optional Prometheus (9091) + Grafana (3000).

## Common Development Patterns

### Adding a Storage Engine
1. Implement `UnifiedStorageEngine` in `src/storage/engines/impls/<name>/`
2. Use `compute::quantization::unified` for quantization
3. Use `UnifiedCachingFilesystem` for I/O
4. Register in `src/storage/engines/factory.rs`
5. Add tests in `tests/engines/`

### Modifying API Endpoints
1. Update `proto/proximadb.proto` or `proto/proximadb/v1/*.proto`
2. Run `cargo build` (regenerates proto types automatically via build.rs)
3. Add route in `src/network/rest/v1/` or implement handler logic in `src/api_handlers/`
4. Update corresponding service implementations in `src/network/grpc/`
5. Add tests in `tests/api_consistency_test.rs` or `tests/api_consistency_comprehensive.rs`

### Important Patterns
- **Memory**: Arc-based zero-copy sharing, memmap2 for large files, Rayon for parallel search
- **Testing**: Use `--test-threads=1` for tests needing ports; clean `/tmp/proximadb*` between runs
- **SKS/AXIS**: `OrionBackedEntityStore` defaults to a globally registered `AxisManager` (set via `SharedServices`). When constructing stores manually in tests, call `set_global_axis_manager(...)` or `with_axis_manager(...)` to avoid falling back to brute-force search.
- **Observability**: Prometheus metrics at `/metrics/prometheus`; regression test in `src/network/tests/metrics_tests.rs`
- **Feature surfaces**: Optional AI, sales, tenant, executive endpoints are feature-gated — see `docs/feature_toggles.md` to keep default binaries lean
- **Graph RAG**: Modular Graph RAG (RGL) with multiple engine implementations in `src/graph/rag/engine_impls.rs`
- **Query Interfaces**: SQL (default), AQL (`src/query/aql/`), Natural Language (`src/query/nl/`)

## Runtime Performance

- **SIMD**: Runtime detection for AVX2/AVX512/NEON with scalar fallback
- **Parallel Search**: Rayon for parallel vector operations
- **Memory Mapping**: memmap2 for large file access
- **Quantization**: Unified quantization in `compute/quantization/unified.rs`
- **Metrics**: Prometheus endpoint at `/metrics/prometheus`
- **Logs**: Component-specific logging via `RUST_LOG=proximadb::<component>=level`
- **Traces**: Distributed tracing support in `src/observability/`

## Troubleshooting

**Common warnings** (not errors): "Collections found but no WAL entries" (normal pre-insert), "Storage engine registration warning" (expected at startup), "Port already in use" (use `lsof -i :5678` to find process).

Test timeouts usually mean resource contention — use `--test-threads=1`.

## Anchoring Artifacts (Source of Truth)

| Artifact | Path |
|----------|------|
| Vision & PRD | `docs/00-product/VISION.adoc`, `docs/00-product/PRD.adoc` |
| Architecture | `docs/concepts/architecture.adoc` |
| Strategic Roadmap | `docs/_internal/roadmap/STRATEGIC_ROADMAP.adoc` |
| Feature Dashboard | `docs/_internal/roadmap/MASTER_FEATURE_DASHBOARD.adoc` |
| Technical Debt | `docs/10-quality/TECHNICAL_DEBT.adoc` (TD-001 through TD-042) |
| Design Patterns | `docs/12-design/DESIGN_PATTERNS.adoc` |
| Code Coverage | `docs/10-quality/code-coverage-report.adoc` |
| Implementation Guides | `docs/09-roadmap/implementation/` |
