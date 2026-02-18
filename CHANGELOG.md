# Changelog

All notable changes to ProximaDB will be documented in this file.

## [0.2.0] - 2025-12-28

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
