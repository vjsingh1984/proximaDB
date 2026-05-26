# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Quick Reference

| Resource | Value |
|----------|-------|
| **Current Version** | `v0.2.0` (in-development) |
| **Next Release Target** | **v0.2** (not yet published — pre-release work targets this release) |
| **Default Ports** | 5678 (unified REST/gRPC), 5679 (gRPC multi-port), 5433 (PostgreSQL wire), 5680 (Arrow Flight) |
| **Default Data** | `/tmp/proximadb/` |
| **Config File** | `config/config.toml` |
| **Health Check** | `curl http://localhost:5678/health` |
| **WAL Manifest** | `/tmp/proximadb/manifest/manifest_*.jsonl` |
| **Main Branch** | `main` |

> **Release framing**: pre-release work in `docs/_internal/status/PRE_RELEASE_FOUNDATIONS_2026_05_26.adoc` and related session docs targets **v0.2** (the next release). Anything labeled "v0.3" / "v0.4" refers to releases AFTER v0.2 (post-v0.2 cleanup, future features). The deprecated gRPC service shims marked `// DEPRECATED: ... removed in version 0.3.0` will be removed in the 0.3.0 release that follows v0.2.

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

### Architecture Mandates

Keep this section short; detailed rules live in the referenced docs.

- Use [docs/12-design/README.adoc](docs/12-design/README.adoc) as the architecture index.
- New internal/durable contracts use `ProximaRecord` + `ProximaType`/`ProximaValue`; legacy `VectorRecord`, `SqlValue`, `SqlObject`, and protocol DTOs are edge adapters only.
- Protocols and modalities are facades, not durable authorities. They lower into xCatalog, canonical records, shared algebra, and canonical WAL.
- Durable authority stays in xCatalog + WAL/log/manifest + `ProximaRecord` + policy/RLS + version/time/provenance. PAX, LSM, columnar, ANN, JSON, graph topology, observability, Arrow/Parquet/Iceberg/Delta/Hudi are layouts, projections, adapters, or explicit external-authority modes.
- OLTP/OLAP/HTAP/MPP route decisions must be cataloged and explainable with `authority_mode`, `workload_profile`, `storage_specialization`, `freshness_sla`, `compute_route`, `partitioning`, `isolation_profile`, and `policy_boundary`. Reject unsafe, stale, lossy, or policy-violating routes.
- Router/multiplexer shape is a control-plane boundary: route once per plan/fragment/split, emit typed `RoutedExecutionPlan` plus unified `EXPLAIN`, then dispatch to leaf executors/readers without per-row route recomputation.
- Generated code must not create hidden durable authority. New routes/readers/writers/projections/adapters must declare authority mode, policy boundary, freshness behavior/state, repair source, rejected-route reasons, and support maturity before default enablement.
- Iceberg/Delta/Hudi/Parquet paths are open-format interoperability modes. Register them in xCatalog as publications, imports, federated reads, or explicit external-authoritative assets; do not treat table logs/files as Proxima-owned hot authority unless the canonical WAL/record path owns the commit.
- Workspace changes follow the stable map `Foundation -> Contracts -> Modality Runtime -> Cross-Model Query Runtime -> Platform Runtime -> Apps/Bindings`; add crates only for real dependency or ownership payoff.
- Before touching records/types/catalog/storage/WAL/query/RLS/open formats/pgwire/Arrow/workspace code, read the relevant docs below and cite doc/ADR ids in PRs.

Architecture references:

- [roadmap/MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc](roadmap/MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc) - record/type/algebra/storage/RLS internals and sticky ADRs.
- [docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc](docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc) - stacked durability and modality convergence.
- [docs/12-design/COMPETITIVE_OLTP_OLAP_MPP_TRAJECTORY_2026_05_20.adoc](docs/12-design/COMPETITIVE_OLTP_OLAP_MPP_TRAJECTORY_2026_05_20.adoc) - OLTP/OLAP/HTAP/MPP route-map and design knobs.
- [docs/12-design/RELATIONAL_STORAGE_FORMAT_AND_INTEROPERABILITY_2026_05_19.adoc](docs/12-design/RELATIONAL_STORAGE_FORMAT_AND_INTEROPERABILITY_2026_05_19.adoc) - PAX/MVCC/open-format storage shape.
- [docs/12-design/RELATIONAL_PGWIRE_DML_COMPUTE_BLUEPRINT_2026_05_20.adoc](docs/12-design/RELATIONAL_PGWIRE_DML_COMPUTE_BLUEPRINT_2026_05_20.adoc) - active pgwire DML and compute-routing tracker.
- [docs/12-design/OPEN_FORMAT_CATALOG_2026_05_17.adoc](docs/12-design/OPEN_FORMAT_CATALOG_2026_05_17.adoc) - open table authority modes and catalog contracts.
- [docs/12-design/adr/ADR-004-unified-explain-contract.adoc](docs/12-design/adr/ADR-004-unified-explain-contract.adoc) - unified EXPLAIN and route/write-plan explanation contract.
- [roadmap/techdebt/WORKSPACE_REFACTOR_PLAN_2026_05_07.adoc](roadmap/techdebt/WORKSPACE_REFACTOR_PLAN_2026_05_07.adoc) - workspace boundaries.

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
