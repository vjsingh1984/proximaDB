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
- Fast inner-loop targets: `make check-fast` (type-check only, no codegen/link) and `make test-fast` (nextest unit suite, parallel; falls back to `cargo test --lib --test-threads=6` if nextest is missing). One-shot install via `make install-fast-tools` (adds `cargo-nextest` and `cargo-watch`).
- For targeted Rust work, prefer `cargo test --lib`, `cargo test --test <name>`, `cargo nxlib` (nextest, parallel), or `cargo test --features test-quick|test-standard|test-full`.
- The dev/test profile uses `debug = "line-tables-only"` for fast incremental link (panic backtraces keep file:line; lldb variable inspection is unavailable — flip back to `debug = true` in `[profile.dev]` only when needed). `.cargo/config.toml` enforces `jobs = 3` and `RUST_TEST_THREADS = "1"` to stay OOM-safe on 10-core hosts; the fast-loop targets bypass the latter via nextest, while the inner `--test-threads=` flag overrides it for cargo test.
- When touching gated code, build or test with the relevant features explicitly: `experimental-engines`, `distributed-graph`, `tiered-graph`, `datafusion-integration`, `llm-joins`, `experimental-cdc-connectors`, `python`, `java`, `nodejs`, `c_ffi`, `aws`, `azure`, or `gcp`.
- Python SDK tests: `cd clients/python && PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python PYTHONPATH=$PWD/src python -m pytest`.
- Python embedded build/test: `cd clients/python-embedded && maturin develop --features python`, then `pytest`.
- UI workflow: `cd ui && npm start`, `npm test`, or `npm run build`.
- If `proto/` changes, regenerate Python gRPC stubs with `cd clients/python && make gen-proto`.

## Coding Style & Naming Conventions
- Rust uses edition 2024 and `cargo fmt` defaults. Keep modules focused, prefer explicit public types, and reuse shared abstractions in `core/`, `schema/`, or service-layer modules instead of introducing near-duplicate structs.
- Extend existing traits, services, proto contracts, caches, and query/graph orchestration layers before adding new infrastructure. Prefer refactoring a directionally aligned capability into the current abstraction over creating a parallel code path for the same concept.
- Treat duplicated concepts as design debt. If a new requirement overlaps an existing engine, router, planner, handler, or metadata path, converge the behavior into the canonical implementation rather than layering patchwork adapters on top.
- Architecture source of truth is reference-doc first, not duplicated here. Use `docs/12-design/README.adoc` as the entry point.
- Core invariant: new durable/internal contracts use `ProximaRecord` + `ProximaType`/`ProximaValue`; legacy v1 `VectorRecord`, `SqlValue`, `SqlObject`, and protocol DTOs are edge adapters only.
- Protocols and modalities are facades. SQL/pgwire, REST/gRPC, Arrow Flight, SDK/embedded, document, graph, vector, and observability paths lower into xCatalog, canonical records, shared algebra, and canonical WAL.
- Durable authority stays in xCatalog + WAL/log/manifest + `ProximaRecord` + policy/RLS + version/time/provenance. PAX, LSM, columnar, ANN, JSON, graph topology, observability, Arrow/Parquet/Iceberg/Delta/Hudi are layouts, projections, adapters, or explicit external-authority modes.
- Competitive OLTP/OLAP/HTAP/MPP route decisions must be cataloged and explainable: `authority_mode`, `workload_profile`, `storage_specialization`, `freshness_sla`, `compute_route`, `partitioning`, `isolation_profile`, and `policy_boundary`. Reject unsafe, stale, lossy, or policy-violating routes.
- Treat the router as a standalone control-plane planner/multiplexer boundary: route once per plan/fragment/split, emit typed `RoutedExecutionPlan`/`EXPLAIN` metadata, then dispatch to leaf executors/readers without per-row route recomputation.
- LLM-generated code must not add hidden authority. New routes, readers, writers, projections, or external adapters must declare authority mode, policy boundary, freshness behavior/state, repair source, rejected-route reasons, and support maturity before default enablement.
- Open formats are interoperability contracts, not implicit authority. Iceberg/Delta/Hudi/Parquet paths must be registered as publications, imports, federated reads, or explicit external-authoritative assets in xCatalog.
- Workspace changes follow `roadmap/techdebt/WORKSPACE_REFACTOR_PLAN_2026_05_07.adoc`: stable map is `Foundation -> Contracts -> Modality Runtime -> Cross-Model Query Runtime -> Platform Runtime -> Apps/Bindings`; add crates only for real dependency or ownership payoff.
- Before touching records, types, catalog, storage, WAL/recovery, query lowering, modality durability, RLS, open formats, pgwire, Arrow Flight, or workspace boundaries, read the relevant docs below and cite the specific doc/ADR in PRs.
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

## Architecture Reference Docs
- `docs/12-design/README.adoc` - canonical architecture index.
- `roadmap/MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc` - record/type/algebra/storage/RLS internals and sticky ADRs.
- `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc` - stacked durability and modality convergence mandate.
- `docs/12-design/COMPETITIVE_OLTP_OLAP_MPP_TRAJECTORY_2026_05_20.adoc` - OLTP/OLAP/HTAP/MPP route-map and design knobs.
- `docs/12-design/RELATIONAL_STORAGE_FORMAT_AND_INTEROPERABILITY_2026_05_19.adoc` - PAX/MVCC/open-format storage shape.
- `docs/12-design/RELATIONAL_PGWIRE_DML_COMPUTE_BLUEPRINT_2026_05_20.adoc` - active pgwire DML and compute-routing tracker.
- `docs/12-design/OPEN_FORMAT_CATALOG_2026_05_17.adoc` plus ADR-007/008/009/010 - Iceberg REST, OLTP catalog backends, schema modes, and PAX block decisions.
- `docs/12-design/adr/ADR-004-unified-explain-contract.adoc` - unified EXPLAIN and route/write-plan explanation contract.
