# init.md — ProximaDB

## Project Overview

ProximaDB is a high-performance, multi-model vector database written in Rust (Edition 2024) with Python and Node.js client SDKs. It supports multiple access protocols—REST/gRPC, PostgreSQL wire protocol, and Arrow Flight—and provides vector similarity search, graph database queries, metadata filtering, and AI embedding integration. The project targets ML/AI engineers and database operators needing embedded or standalone vector storage at scale. Python SDK version is 0.2.0 (Beta).

## System Flow

```
Client → Network Layer (REST/gRPC/PG Wire/Arrow Flight) → Auth → API Handlers → Query Engine → Storage Engine (Raptor) → WAL → Disk
```

## Package Layout

| Path | Type | Description |
|------|------|-------------|
| `src/` | Library root | Core Rust crate — storage, query, network, WAL, observability |
| `src/storage/engines/raptor/` | Storage engine | Raptor vector storage engine with bloom filters |
| `src/storage/persistence/write_ahead_log/` | Persistence | WAL subsystem with background maintenance |
| `src/storage/cache/` | Cache | Caching layer with health monitoring |
| `src/query/federated/` | Query engine | Federated query execution with catalog management |
| `src/api_handlers/` | API handlers | REST/gRPC endpoint handlers (including AI endpoints) |
| `src/network/auth/` | Auth | Authentication service |
| `src/observability/` | Observability | Alerting and monitoring subsystem |
| `src/operations/` | Operations | Backup and maintenance operations |
| `proto/` | Protobuf | Protobuf definitions; watched by `build.rs` for prost-build regeneration |
| `config/` | Config | Default `config.toml` |
| `clients/python/` | Python SDK | PyO3/maturin-based Python bindings with native module |
| `clients/rust/` | Rust SDK | Workspace member Rust client (`--features client`) |
| `clients/nodejs-embedded/` | Node.js SDK | TypeScript/Node embedded client |
| `tests/` | Tests | Integration, engine, WAL, and graph tests |
| `benches/` | Benchmarks | Criterion benchmarks |
| `ui/` | Dashboard | Web UI (npm-based) |
| `demo/` | Demos | Example workflows including BERT embedding service |
| `scripts/` | Scripts | Automation and helper scripts |
| `docs/` | Documentation | Architecture docs and internal reports |

## Key Entry Points

| Component | Type | Path:line | Description |
|-----------|------|-----------|-------------|
| `proximadb-server` | Binary | `src/bin/proximadb-server.rs` | Main server entry point (ports 5678–5680) |
| `proximadb-bench` | Binary | `src/bin/proximadb-bench.rs` | Consolidated benchmark tool |
| `proximadb-migrate` | Binary | `src/bin/proximadb-migrate.rs` | Schema migration utility |
| `ManagerRegistry` | Struct | `src/storage/persistence/write_ahead_log/mod.rs:390` | Registry coordinating all WAL manager instances |
| `WriteAheadLogManager` | Struct | `src/storage/persistence/write_ahead_log/mod.rs:1226` | Core WAL write/replay engine |
| `BackgroundMaintenanceManager` | Struct | `src/storage/persistence/write_ahead_log/background_manager.rs:53` | WAL compaction and cleanup |
| `Federated` (CatalogManager) | Struct | `src/query/federated/mod.rs:137` | Federated query coordinator with catalog |
| `ArtusBloomManager` | Struct | `src/storage/engines/raptor/artus_bloom.rs:61` | Bloom filter index in Raptor engine |
| `AuthService` | Struct | `src/network/auth/mod.rs:163` | Authentication and authorization gateway |
| `AIServiceState` | Struct | `src/api_handlers/ai_endpoints.rs:24` | AI/embedding endpoint handler |
| `AlertManager` | Struct | `src/storage/cache/health_monitor.rs:66` | Cache health monitoring and alerting |
| `AlertingService` | Struct | `src/observability/alerting/mod.rs:34` | System-wide observability alerts |
| `BackupManager` | Struct | `src/operations/backup/mod.rs:24` | Backup and restore operations |
| `BERTEmbeddingService` | Class | `demo/utils/bert_embedding_service.py:24` | Python demo embedding generation |

## Architecture Patterns

- **Multi-protocol front-end**: A unified server exposes REST+gRPC (5678), gRPC multi-port (5679), PostgreSQL wire protocol (5433), and Arrow Flight (5680) from a single binary, routing through a shared auth and handler layer.
- **WAL-first persistence**: All writes flow through `WriteAheadLogManager` with a `ManagerRegistry` coordinating per-collection logs and `BackgroundMaintenanceManager` handling compaction. WAL manifests stored as JSONL at `/tmp/proximadb/manifest/`.
- **Raptor storage engine**: Pluggable storage engine architecture with the Raptor engine as primary, using `ArtusBloomManager` for probabilistic filter indexes alongside vector indexes.
- **Proto-driven API**: `build.rs` watches `proto/` and regenerates Rust code via `prost-build`, making protobuf the source of truth for the gRPC surface.
- **Dual crate output**: The library crate produces both `rlib` (for the server binary and Rust SDK) and `cdylib` (for PyO3 Python bindings via maturin).
- **Dev/test profile parity**: `dev` and `test` profiles share `opt-level=0` for 100% artifact reuse; dependencies like arrow/parquet compile at `opt-level=2` even in dev. A `release-server` profile provides LTO-optimized server builds.
- **Co-located unit tests**: Inline `#[cfg(test)] mod tests` blocks in source files test private APIs; standalone integration tests in `tests/` cover cross-module concerns. CI uses feature flags (`test-quick`, `test-standard`, `test-full`) to gate test categories.
- **Federated query with catalog**: The `Federated` coordinator holds a `CatalogManager` to resolve and dispatch queries across collections, enabling cross-collection graph and vector operations.

## Development Commands

```bash
# Build
cargo build                              # Debug
cargo build --release                    # Release (opt-level=3, LTO)
cargo build --profile release-server     # Optimized server build
cargo check --all-targets                # Fast syntax check

# Run server
cargo run --bin proximadb-server         # Debug (port 5678)
cargo run --release --bin proximadb-server

# Test
cargo test                               # All tests
cargo test --lib                         # Inline unit tests only
cargo test --test integration            # Integration tests
cargo test --test graph_integration_test # Graph tests
cargo test -- --test-threads=1           # Sequential (port-binding tests)
RUST_LOG=debug cargo test test_name -- --nocapture --test-threads=1

# Quality
cargo fmt && cargo clippy -- -D warnings && cargo test

# Python SDK
cd clients/python && pip install -e .
PYTHONPATH=clients/python/src pytest clients/python/tests/ -v

# Rust SDK
cd clients/rust && cargo build --features client

# Web UI
cd ui && npm install && npm start

# Benchmarks
cargo bench
cargo run --bin proximadb-bench

# Full check via Makefile
make check
```

## Dependencies

**Rust** (Cargo workspace, root + `clients/rust`): arrow, parquet, prost/prost-build, tokio, tonic. **Python SDK** (pip/maturin): numpy≥1.21, pytest, maturin≥1.4. **Node SDK**: TypeScript with native bindings.

## Configuration

Primary config: `config/config.toml`. Default data directory: `/tmp/proximadb/`. Server ports (5678–5680, 5433) are configurable via config file or CLI flags. WAL manifests written to `/tmp/proximadb/manifest/` as JSONL files. Health check: `curl http://localhost:5678/health`.

## Codebase Scale

~1.49M LOC across 2,679 files (2,391 source, 288 config). Primarily Rust (1,863 files) with Python (467), TypeScript (26), Go (17), and client SDKs in multiple languages.

Run `/init --update` to refresh after code changes.