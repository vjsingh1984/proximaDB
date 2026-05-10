# init.md — ProximaDB

## Project Overview

ProximaDB is a high-performance, multi-model vector database written in Rust (Edition 2024) with Python (PyO3/maturin) and Node.js client SDKs. It exposes vector similarity search, graph queries, metadata filtering, and AI embedding integration through four concurrent protocols: REST/gRPC, PostgreSQL wire protocol, and Arrow Flight. The project targets ML/AI engineers and database operators needing embedded or standalone vector storage at scale. Python SDK is at v0.2.0 (Beta).

## System Flow

```
Client → Network Layer (REST/gRPC/PG Wire/Arrow Flight) → AuthService → API Handlers → Federated Query Engine → Raptor Storage Engine → WAL → Disk
```

## Package Layout

| Path | Type | Description |
|------|------|-------------|
| `src/` | Library root | Core Rust crate — storage, query, network, WAL, observability |
| `src/storage/engines/raptor/` | Storage engine | Raptor vector storage engine with bloom filter indexes |
| `src/storage/persistence/write_ahead_log/` | Persistence | WAL subsystem with background maintenance and compaction |
| `src/query/federated/` | Query engine | Federated query execution with catalog and optimizer |
| `src/api_handlers/` | API handlers | REST/gRPC endpoint handlers including AI endpoints |
| `src/network/auth/` | Auth | Authentication and authorization service |
| `src/observability/` | Observability | Alerting and monitoring subsystem |
| `src/operations/` | Operations | Backup and maintenance operations |
| `src/embedded/` | Embedded mode | Embedded runtime for in-process usage |
| `crates/foundation/proximadb-proto/` | Proto crate | Generated Protobuf/gRPC types via prost-build |
| `proto/` | Protobuf defs | Source-of-truth `.proto` files; watched by `build.rs` |
| `clients/python/` | Python SDK | PyO3/maturin native bindings with chunking strategies |
| `clients/rust/` | Rust SDK | Workspace-member Rust client (`--features client`) |
| `clients/nodejs-embedded/` | Node.js SDK | TypeScript embedded client |
| `config/` | Config | Default `config.toml` |
| `benches/` | Benchmarks | Criterion benchmarks |
| `tests/` | Tests | Integration, engine, WAL, and graph tests |
| `demo/` | Demos | Example workflows (BERT embedding, progressive search, RAG) |
| `ui/` | Dashboard | Web UI (npm-based) |
| `scripts/` | Scripts | Build automation, workspace boundary checks, measurements |

## Key Entry Points

| Component | Type | Path:line | Description |
|-----------|------|-----------|-------------|
| `proximadb-server` | Binary | `src/bin/proximadb-server.rs` | Main server entry (ports 5678–5680) |
| `proximadb-bench` | Binary | `src/bin/proximadb-bench.rs` | Consolidated benchmark tool |
| `proximadb-migrate` | Binary | `src/bin/proximadb-migrate.rs` | Schema migration utility |
| `ManagerRegistry` | Struct | `src/storage/persistence/write_ahead_log/mod.rs:390` | Registry coordinating all WAL manager instances |
| `WriteAheadLogManager` | Struct | `src/storage/persistence/write_ahead_log/mod.rs:1226` | Core WAL write/replay engine |
| `BackgroundMaintenanceManager` | Struct | `src/storage/persistence/write_ahead_log/background_manager.rs:53` | WAL compaction and cleanup |
| `Federated` (CatalogManager) | Struct | `src/query/federated/mod.rs:137` | Federated query coordinator with catalog |
| `ArtusBloomManager` | Struct | `src/storage/engines/raptor/artus_bloom.rs:61` | Bloom filter index in Raptor engine |
| `AuthService` | Struct | `src/network/auth/mod.rs:163` | Auth gateway for all protocols |
| `AIServiceState` | Struct | `src/api_handlers/ai_endpoints.rs:24` | AI/embedding endpoint handler |
| `AlertingService` | Struct | `src/observability/alerting/mod.rs:34` | System-wide observability alerts |
| `BackupManager` | Struct | `src/operations/backup/mod.rs:24` | Backup and restore operations |
| `ProximaDBClient` | Class | `clients/python/src/proximadb_sdk/unified_client.py:84` | Python SDK hub class (125 connections) |
| `QueryOptimizer` | Module | `src/query/federated/optimizer/mod.rs:1` | Query plan optimization (206 symbols) |

## Architecture Patterns

- **Multi-protocol front-end**: Single binary exposes REST+gRPC (5678), gRPC multi-port (5679), PG wire protocol (5433), and Arrow Flight (5680), routing through a shared `AuthService` and unified handler layer.
- **WAL-first persistence**: All writes flow through `WriteAheadLogManager`; `ManagerRegistry` coordinates per-collection logs; `BackgroundMaintenanceManager` handles compaction. Manifests stored as JSONL at `/tmp/proximadb/manifest/`.
- **Raptor storage engine with bloom indexes**: Pluggable engine architecture — Raptor is primary, using `ArtusBloomManager` for probabilistic filter indexes alongside vector indexes.
- **Proto-driven API surface**: `build.rs` watches `proto/` and regenerates Rust types via `prost-build` into `crates/foundation/proximadb-proto/`. The generated `proximadb.v1.rs` (532 symbols) is the largest file — treat it as read-only.
- **Dual crate output**: The library produces both `rlib` (for server binary and Rust SDK) and `cdylib` (for PyO3 Python bindings via maturin).
- **Python SDK hub pattern**: `ProximaDBClient` (125 connections) is the central facade in the Python SDK, delegating to chunking strategies (`code.py` — 278 symbols), multimodal query, and models modules.
- **Federated query with optimizer**: `Federated` struct at `src/query/federated/mod.rs:137` coordinates query execution with a dedicated optimizer module (`optimizer/mod.rs` — 206 symbols) handling plan transformation.
- **Workspace boundary discipline**: `scripts/check_workspace_boundaries.py` enforces layer rules between crates; run `make workspace-boundaries-check` before cross-crate changes.

## Architecture Evidence

- **Graph scale**: `402,696` nodes and `3,725,996` edges were available for architecture analysis.
- **Statement-level flow evidence**: `3,608,505` CFG/CDG/DDG edges (branching ratio `9.25`) back the control- and data-flow claims with graph data.

## Development Commands

```bash
# Full build and test
make all                          # build + test
make build                        # debug build
make build-release                # release build
make build-server                 # server binary only

# Testing
make test                         # all tests (Rust + Python)
make test-rust                    # cargo test
make test-integration             # integration tests only
make test-python                  # pytest for Python SDK

# Linting & quality
make check                        # fmt + clippy + test
make fmt                          # cargo fmt
make clippy                       # cargo clippy

# Benchmarks
make benchmark                    # all benchmarks
make benchmark-vector             # vector-specific benchmarks

# Python SDK development
cd clients/python && pip install -e ".[dev]" && pytest

# Release
make release                      # clean + build-server + test + benchmark
```

## Dependencies

**Rust** (Cargo): tokio, prost/prost-build (gRPC), tonic, arrow, serde, criterion — workspace with multiple crates including `proximadb-proto`. **Python** (pip/maturin): numpy≥1.21 (core), pytest/pytest-asyncio/mypy/ruff (dev).

## Configuration

- Primary config: `config/config.toml` loaded at server startup.
- WAL manifests: JSONL files under `/tmp/proximadb/manifest/`.
- Dev/test profile parity enforced in `Cargo.toml` — maintain alignment when changing profiles.

## Codebase Scale

~1.6M LOC across 3,155 files (Rust: 1,988 files dominant, Python: 718). 402,696 symbols, 3,725,996 graph relationships, 96.8% statement-level CFG coverage.

---

Run `/init --update` to refresh after code changes.
