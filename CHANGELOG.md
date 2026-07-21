# Changelog

All notable changes to ProximaDB will be documented in this file.

## [0.3.0] - 2026-07-21

### Security
- Arrow Flight export path-binding: export file paths are canonicalized + confined to the selected collection's data directory, closing a path-traversal / cross-collection-read gap (#1121).

### API & Observability
- Spec↔router drift smoke gate: every OpenAPI spec path is asserted to resolve on the production router (#1103).
- Cache metrics exposed in `/metrics/prometheus` + OTLP: `proximadb_cache_hits_total{tier}`, misses, evictions, entries, bytes (#1115).
- `query-conformance-check` release gate: release-cut now runs the TPC-H/TPC-DS pgwire ratchets (#1121).

### SDK
- Python SDK facade fixes: short collection names accepted (S2a); `search()` delegates to the REST transport instead of silently falling back to a client-side local store (S2b) (#1113).
- ADR-068 SDK codegen doctrine: hand-maintained facade over narrow generated types; generated client as a CI oracle; correctness gate is a behavioral round-trip (#1112).

### Storage & Architecture
- ADR-069 / TD-WAL-1: detachable local-disk WAL + tiered flush design (reapable to spot instances for IOPS-latency reduction + cost control).
- TD-RDSTRAT-9: collection read-cost envelope — closes the loop between file geometry, region cache, and compaction (#1120).
- ORION engine decoupling (moves 6a–6f): graph-engine trait, WAL/cache/fs/distance deps extracted to leaf crates.

### Infrastructure
- sandhi-core integrated from crates.io (0.1.1) — registry release replaces the git-rev pin (#1122).
- io-trace delivery hardening across crashes (#1093).

## [0.2.2] - 2026-07-04

### Storage
- Always-PAX flush/compaction for the SST engine (ADR-049 M1-3); PAX segment compaction; AXIS rebuild from SST on index-store loss.

### Statistics & routing
- Canonical segment-statistics contract (ADR-042) + neutral envelope (ADR-037) + freshness floor (ADR-038); ADR-050 cost-based routing recorded.

### API
- gRPC v2 GetRecord Python stubs regenerated to match the proto; two-surface SQL model (pgwire + JWT gRPC ExecuteQuery) documented — neither deprecated.

### OSS adoption
- First-run funnel fixed (docker refs -> published vjsingh1984/proximadb, :latest live multi-arch); README status reconciled to SUPPORTED_SURFACE ("the contract wins"); OPEN_CORE.md + COMPETITIVE_LANDSCAPE.adoc added.

### Hygiene
- Clippy real-bug tail + mechanical sweep; attribution/doc-authority CI gates; deterministic-commit contract.

## [0.2.0] - 2026-02-22

### 🎉 Major Release: Platform Packages

This release introduces **native platform packages** for Linux and Windows, making installation easier than ever.

### Platform Packages

#### Linux
- **RPM Packages** (Red Hat/CentOS/Fedora 8+)
  - `proximadb-0.2.0-1.el8.x86_64.rpm`
  - Systemd service integration
  - Automatic user and directory creation
  - Installation: `sudo rpm -ivh proximadb-0.2.0-1.el8.x86_64.rpm`

- **DEB Packages** (Debian/Ubuntu)
  - `proximadb_0.2.0_amd64.deb`
  - Systemd service integration
  - Configuration management
  - Installation: `sudo dpkg -i proximadb_0.2.0_amd64.deb`

#### Windows
- **MSI Installer** (Windows 10+)
  - `proximadb-0.2.0-x64.msi`
  - Installation to `C:\Program Files\ProximaDB\`
  - Start menu shortcuts
  - PATH environment variable setup
  - Installation: `msiexec /i proximadb-0.2.0-x64.msi`

### Release Infrastructure

- ✅ Automated platform package builds (RPM via fpm, DEB via fpm, MSI via WiX v4)
- ✅ Multi-platform binary support (Linux x86_64, Windows x86_64)
- ✅ Pre-release CI validation workflow
- ✅ Automated release workflow with GitHub Releases integration
- ✅ Version consistency automation across all packages
- ✅ PyPI and crates.io publishing automation

### Installation Improvements

- **Native package manager integration** for Linux distributions
- **Systemd service support** with automatic start/stop
- **Configuration file management** (/etc/proximadb/config.toml)
- **Data directory creation** (/var/lib/proximadb)
- **Log directory management** (/var/log/proximadb)

### Documentation

- Installation guides for all platforms
- Platform package documentation
- Release preparation and validation procedures
- Automated changelog generation

### Known Limitations

- **macOS packages (DMG)**: Not available in v0.2.0 due to ring crate CPU feature detection issues on CI. Planned for v0.2.1.
- **Python embedded wheels**: Disabled for v0.2.0 (clients/python-embedded is pure Python, not Rust)
- **ARM64 packages**: Planned for future releases

### Testing

- Platform package builds validated (RPM: 41s, DEB: 46s, MSI: 54s)
- Installation testing on Linux (RHEL/Ubuntu) and Windows
- Pre-release CI validation passed
- Dry-run release validation completed successfully

### Migration from v0.1.x

No breaking changes from v0.1.x. Existing configurations and data are compatible.

### Contributors

Thanks to all contributors who made this release possible!

---

## [0.1.5] - Previous Release

### Major Features

#### Unified Multi-Model Storage Architecture
Complete implementation of the 14-phase unified storage architecture plan.

#### Document Storage (Phase 1A)
- JSON document storage with WAL-backed durability
- JSON path indexing and queries (`$.path.to.field`)
- Full-text search integration with Tantivy
- Array indexing for nested document queries

#### Observability Pipeline (Phase 1B)
- High-throughput log ingestion (1M+ logs/sec target)
- 6 SIEM adapter formats: OTLP, Syslog, Fluent, CEF/LEEF, OCSF, HTTP JSON
- Time-partitioned storage with hot/warm/cold tiering
- Metric aggregation with downsampling
- Trace assembly and span relationships

#### PostgreSQL Wire Protocol (Phase 2)
- Full v3.0 protocol compatibility
- DDL support: CREATE/DROP/ALTER TABLE, INDEX, COLLECTION
- DML support: INSERT, UPDATE, DELETE with prepared statements
- Extended query protocol with Bind/Execute
- COPY protocol for bulk imports (Text, CSV, Binary, Arrow IPC)

#### Unified Query Layer (Phase 3)
- Cross-model query decomposition and execution
- Parallel execution with configurable concurrency
- 5 fusion strategies: Intersection, Union, RRF, Weighted, First-With-Filter
- Vector + Graph + Document + Observability joins

#### Multi-Tenant Isolation (Phase 6.1)
- Tenant-aware storage paths
- X-Tenant-ID header and JWT claim extraction
- Per-tenant resource isolation

#### Distributed Query Coordination (Phase 6.2)
- Shard-aware query routing
- Parallel remote execution with retry logic
- Result aggregation strategies

#### Auto-Tiering Policy Engine (Phase 6.3)
- Hot/Warm/Cold/Archive performance tiers
- Access pattern tracking with hotness scoring
- Policy DSL for age, access, and size-based rules
- Migration coordination with priority queues

#### Multi-Model Transaction Coordinator (Phase 7)
- ACID transactions across Vector, Document, Graph, Observability stores
- 5 isolation levels: ReadUncommitted to Serializable
- 2PC protocol with participant coordination
- Savepoints and nested transaction support

#### Cross-Model Joins (Phase 10)
- Hash-based join execution
- Inner, Left Outer, Semi, Anti join types
- StartNodeSpec resolution for graph integration
- Query optimization with selectivity estimation

#### SQL Parser Upgrade (Phase 10.4)
- EXISTS/NOT EXISTS subqueries
- LIKE/ILIKE operators
- BETWEEN expressions
- IS NULL/IS NOT NULL
- IN list expressions
- CROSS JOIN support

### New Components

#### Unified Port Architecture (Phase 14)
- Single port (5678) for REST, gRPC, and Arrow Flight
- Protocol multiplexing with automatic detection
- HTTP/2 support with ALPN negotiation
- Backward-compatible multi-port mode

#### Web UI Dashboard (Phase 12)
- SQL Query Editor with Monaco Editor
  - ProximaDB SQL syntax highlighting
  - Query history and sample queries
  - Results table with execution metrics
- Graph Explorer with Cytoscape.js
  - 6 layout algorithms (Force-directed, Circle, Grid, etc.)
  - Node/edge filtering and traversal control
  - PNG and JSON export
- Dark/Light theme support
- 10-tab dashboard: Overview, Collections, Query, Graph, Performance, Cache, Security, Alerts, Metrics, Diagnostics

#### Python SDK Enhancements (Phase 13)
- **Graph Analytics**: PageRank, centrality, community detection, pattern matching
- **AutoML Integration**: Engine selection, workload prediction, hyperparameter optimization
- **Observability**: Prometheus metrics, OpenTelemetry tracing, structured logging
- **Multi-Modal Queries**: Unified query builder, semantic joins, graph-vector fusion
- **Security**: OAuth2 token management, RBAC, audit logging, mTLS

### Documentation
- Storage Engine Selection Guide
- Graph Engine Selection Guide
- Unified Port Migration Guide
- Python SDK Guide

### Testing
- 3,560 unit tests passing
- Integration tests for all engines
- Python SDK tests with all 6 storage engines

### Breaking Changes
- Default port changed to unified mode (5678 for all protocols)
- PostgreSQL wire protocol moved to port 5433

## [0.1.5] - Previous Release
- Initial multi-engine vector storage
- ORION graph engine
- Basic REST and gRPC APIs
