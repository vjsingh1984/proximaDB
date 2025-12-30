# init.md

This file provides guidance to Victor when working with code in this repository.

## Project Overview

**proximaDB**: A multi-language vector database engine with support for semantic search, graph analytics, and AI-powered indexing. Built with a modular architecture that supports pluggable storage engines, embedding providers, and graph algorithms.

**Languages**: c (1), cpp (1), csharp (1), elixir (1), go (2), java (4), javascript (5), kotlin (1), php (1), python (388), ruby (1), rust (1505), scala (1), swift (1), typescript (15)

## Package Layout

| Path | Type | Description |
|------|------|-------------|
| `src/` | Active | Core engine source code (Rust, C/C++, Go) |
| `docs/` | Active | Documentation |
| `examples/` | Active | Examples and sample workflows |
| `scripts/` | Active | Automation and helper scripts |
| `tests/` | Active | Unit and integration tests |
| `clients/` | **ACTIVE** | Language-specific SDKs and clients |
| `demo/` | **ACTIVE** | Demo applications and integration examples |

## Key Components

| Component | Type | Path | Description |
|-----------|------|------|-------------|
| AIServiceState | struct | `src/api_handlers/ai_endpoints.rs:24` | Manages AI service state and configuration for LLM integration |
| AlertManager | struct | `src/storage/cache/health_monitor.rs:52` | Monitors system health and triggers alerts for storage engine failures |
| AlertingService | struct | `src/observability/alerting/mod.rs:25` | Centralized alerting service with support for email, Slack, and webhooks |
| ArtusBloomManager | struct | `src/storage/engines/impls/raptor/artus_bloom.rs:61` | Implements bloom filters for fast membership testing in Raptor engine |
| AuthService | struct | `src/network/auth/mod.rs:127` | Handles authentication and authorization for API endpoints |
| AutoMLService | struct | `src/automl/service.rs:167` | AutoML pipeline orchestrator for vector embedding selection and model training |
| BERTEmbeddingService | class | `clients/python/tests/integration/bert_embedding_service.py:24` | Service for generating BERT embeddings from text |
| BackgroundMaintenanceManager | struct | `src/storage/persistence/write_ahead_log/background_manager.rs:53` | Manages background maintenance tasks like compaction and log rotation |
| BaseService | class | `clients/python/tests/chunking/resources/javascript/sample.js:33` | Base class for all Python SDK services, providing common functionality |

## Dependencies

**Core** (1 packages): numpy

## Configuration

- Settings: `.env` → `~/.victor/profiles.yaml` → CLI flags (override order)
- Project context: `.victor/init.md` (regenerate with `victor init --update`)

## Quick Start

```bash
npm install
pytest
pip install -e ".[dev]"
```

## Config Files

- `Cargo.toml`
- `clients/nodejs-embedded/package.json`
- `clients/python-embedded/pyproject.toml`
- `clients/python/pyproject.toml`
- `clients/python/tests/all_databases_results.json`
- `clients/python/tests/chunking/resources/json/sample.json`
- `clients/python/tests/chunking/resources/yaml/sample.yaml`
- `clients/python/tests/graph_benchmark_results.json`
- `clients/python/tests/reports/e2e/bert_flow_1759794976.json`
- `clients/python/tests/reports/e2e/bert_flow_1759799962.json`
- `clients/python/tests/reports/e2e/bert_flow_1759893188.json`
- `clients/python/tests/reports/e2e/bert_flow_1759893259.json`

## Documentation

- `docs/SOLID_VECTOR_FRAMEWORK_REDESIGN.md`
- `CLAUDE.md`
- `demo/CONTRIBUTING.md`
- `clients/python/EMBEDDING_ARCHITECTURE_REDESIGN.md`
- `clients/python/GRAPH_API_TEST_RESULTS.md`
- `clients/python/EMBEDDING_PROVIDERS.md`
- `src/storage/engines/impls/sst/tests/SST_CORE_TESTS_ANALYSIS.md`
- `clients/python/examples/README.md`
- `demo/README.md`
- `demo/MIGRATION_GUIDE.md`

## Codebase Stats

- **864,939** lines of code across **1928** files
- Largest file: `src/proto/proximadb.v1.rs` (16,147 lines)
- Top files by size:
  - `src/proto/proximadb.v1.rs` (16,147 lines)
  - `src/storage/engines/core/formats/proximablocks/block_structures.rs` (6,232 lines)
  - `src/storage/engines/impls/sst/readers/sst_query_engine.rs` (6,100 lines)

## Analyzer Coverage

- **Symbol index**: 41248 symbols across 1928 files (multi-language tree-sitter + regex fallback, includes call sites and references)
- **Code graph**: 7295 nodes, 10780 edges (calls: 1314, refs: 1716, imports: 2399, inheritance: 171, composition: 1); module PageRank and coupling detection ready
  - Hub classes: `LanguageParser` (25 links), `BaseProximaDBTest` (23 links)
  - Module PageRank leaders: `clients/python/src/proximadb_sdk/models.py` (service), `clients/python/src/proximadb_sdk/chunking_strategies/code.py` (intermediary), `clients/python/src/proximadb_sdk/unified_client_v2.py` (service)
  - Graph coverage by language: c (1), cpp (1), csharp (1)
  - Call hotspots: `get` (35 callers), `main` (20 callers), `create_collection` (17 callers)
- **Semantic embeddings**: Tool Embeddings (3 files, 223.3 KB, 1d ago), Task Classifier (0 files, 0 B, never), Conversation Embeddings (11 files, 633.1 KB, 10d ago) (tree-sitter chunking for code spans; tool/intent/conversation caches ready)

## Graph Health

- Nodes: 7295, Edges: 10780 (CALLS: 1314, REFERENCES: 1716, IMPORTS: 2399, INHERITS: 171, COMPOSED_OF: 1)
- Language coverage: c (1), cpp (1), csharp (1), go (35)
- Hub classes: LanguageParser (25 links), BaseProximaDBTest (23 links), ProximaDBError (22 links)
- Module leaders: clients/python/src/proximadb_sdk/models.py (service), clients/python/src/proximadb_sdk/chunking_strategies/code.py (intermediary), clients/python/src/proximadb_sdk/unified_client_v2.py (service)

## Most Imported Modules

*Non-stdlib modules imported most frequently*

- `proximadb_sdk` (362 imports)
- `proximadb` (190 imports)
- `numpy` (143 imports)
- `pytest` (101 imports)
- `google` (64 imports)
- `embedding_utils` (42 imports)
- `grpc` (42 imports)
- `httpx` (41 imports)

## Named Implementations

### Graph Engines

| Name | Location | Description |
|------|----------|-------------|
| **ORION** | `src/graph/engines/orion/algorithms/centrality.rs:81` | Implements senessCentrality algorithm for graph analytics |
| **PULSAR** | `src/graph/engines/pulsar/consensus/mod.rs:153` | Implements consensus for distributed graph state management |
| **QUASAR** | `src/graph/engines/quasar/mod.rs:74` | Core graph engine with support for dynamic query optimization |

### Storage Engines

| Name | Location | Description |
|------|----------|-------------|
| **HELIX** | `src/storage/engines/impls/helix/clustering.rs:21` | Implements PCA-based clustering for vector space optimization |
| **MIGRATION** | `src/storage/engines/migration/migrator.rs:18` | Migrator for upgrading between storage engine versions |
| **NOVA** | `src/storage/engines/impls/nova/batch_operations.rs:19` | Batch operation manager for parallelized storage tasks |
| **ORION** | `src/graph/engines/orion/persistence.rs:41` | OrionSnapshot persistence manager for graph state |
| **QUASAR** | `src/graph/engines/quasar/cache.rs:30` | AccessPatternCache for optimizing graph traversal |
| **RAPTOR** | `src/storage/engines/impls/raptor/adaptive_pxk.rs:33` | Vector selection and indexing engine for Raptor |
| **SST** | `src/storage/engines/impls/sst/blocks.rs:44` | SstRecord for structured storage blocks |
| **SWIFT** | `src/storage/engines/impls/swift/batch_operations.rs:17` | Batch operations for Swift engine |
| **UNIVERSAL** | `src/storage/engines/universal/adapter.rs:33` | HardwareAccelerationManager for CPU/GPU optimization |
| **VIPER** | `src/storage/engines/impls/viper/codebook_sidecar.rs:18` | ViperCodebookSidecarManager for codebook-based vector compression |

### Storage Ops

| Name | Location | Description |
|------|----------|-------------|
| **BASELINE** | `src/storage/engines/core/ops/proximacodec/impls/baseline/decoder.rs:15` | BaselineDecoder for standard vector compression |
| **GPU** | `src/storage/engines/core/ops/proximacodec/impls/gpu/batching.rs:55` | GpuBatchSizer for GPU-accelerated vector operations |
| **SIMD** | `src/storage/engines/core/ops/proximacodec/impls/simd/decoder.rs:26` | SimdDecoder for SIMD vector decoding |

### Storage Providers

| Name | Location | Description |
|------|----------|-------------|
| **LOCAL** | `clients/python/src/proximadb_sdk/embedding_providers/providers/local/bge.py:73` | BGE embedding provider for local models |
| **TESTING** | `clients/python/src/proximadb_sdk/embedding_providers/providers/testing/simulated.py:34` | Simulated embedding provider for testing |

## Performance Hints

*Extracted from docstrings and comments*

- `clients/python/examples/embedding_providers_demo.py`: Performance
Best for: Production retrieval, semantic search
- `clients/python/examples/sks_graph_first_demo.py`: 20ms, Performance Comparison Summary

Shows actual benchmarked perfo
- `clients/python/src/proximadb_sdk/builders/collection.py`: performance collection for ML embeddings")
- `clients/python/src/proximadb_sdk/cache.py`: Performance metrics

Usage:
    pool = ObjectPool(
        fac
- `clients/python/src/proximadb_sdk/chunking.py`: Performance monitoring and metrics via ResourcePool

Performan, performance statistics
- `clients/python/src/proximadb_sdk/chunking_strategies/pipeline.py`: performance for I/O-bound operations
- `clients/python/src/proximadb_sdk/embedding_providers/__init__new.py`: o("gte-qwen")
- `clients/python/src/proximadb_sdk/embedding_providers/core/config.py`: performance scores, and usage requirements

## Architecture

1. **Component Pattern**: Found 28 component components
2. **Config Pattern**: Found 1068 config components
3. **Controller Pattern**: Found 68 controller components
4. **Factory Pattern**: Found 165 factory components
5. **Middleware Pattern**: Found 254 middleware components
6. **Model Pattern**: Found 330 model components
7. **Provider Pattern**: Found 345 provider components
8. **Repository Pattern**: Found 416 repository components

## Code Structure

- 1019 classes
- 1 components
- 1143 enums
- 27835 functions
- 3660 impls
- 30 interfaces
- 95 methods
- 2214 modules
- 5053 structs
- 190 traits
- 8 types

## Setup & Commands

```bash
npm install
pip install -e ".[dev]"
pytest
```

## Learned from Conversations

*Based on 14 sessions, 206 messages*

### Frequently Referenced Files

- `src/api_handlers/ai_endpoints.rs` (1 references)
- `src/storage/cache/health_monitor.rs` (1 references)
- `src/index/axis/management/monitor.rs` (1 references)
- `src/storage/engines/impls/raptor/artus_bloom.rs` (1 references)
- `src/network/auth/mod.rs` (1 references)
- `src/automl/service.rs` (1 references)
- `src/index/axis/management/manager.rs` (1 references)
- `src/index/axis/integration/tiering_manager.rs` (1 references)

### Common Topics

Keywords: component, what

### Frequently Asked Questions

- What are the key components in the project? Summary modules one at a time along with one improvement ...

## Code Graph Insights

*7295 symbols, 10780 relationships*

### Most Important Symbols (PageRank)

| Symbol | Type | Connections |
|--------|------|-------------|
| `get` (clients/python/src/proximadb_sdk/document_processor.py:789) | function | ↓35 ↑0 |
| `main` (demo/sks_demo.py:276) | function | ↓20 ↑0 |
| `create_collection` (tools/benchmarks/performance_comparison.py:10) | function | ↓17 ↑0 |
| `__init__` (demo/setup.py:49) | function | ↓15 ↑0 |
| `insert_vectors` (clients/python/src/proximadb_sdk/unified_client_v2.py:212) | function | ↓15 ↑0 |
| `delete_collection` (clients/python/src/proximadb_sdk/unified_client_v2.py:204) | function | ↓13 ↑0 |

### Hub Classes (High Connectivity)

- `LanguageParser` (clients/python/src/proximadb_sdk/chunking_strategies/code.py:149) - 25 connections
- `BaseProximaDBTest` (clients/python/tests/utils/base_test.py:26) - 23 connections
- `ProximaDBError` (clients/python/src/proximadb_sdk/exceptions.py:22) - 22 connections

### Key Modules (Architecture)

| Module | Role | Connections |
|--------|------|-------------|
| `clients/python/src/proximadb_sdk/models.py` | 🔧 service | ↓14 ↑0 |
| `clients/python/src/proximadb_sdk/chunking_strategies/code.py` | ↔️ intermediary | ↓3 ↑5 |
| `clients/python/src/proximadb_sdk/unified_client_v2.py` | 🔧 service | ↓14 ↑0 |
| `clients/python/src/proximadb_sdk/document_processor.py` | 🔧 service | ↓10 ↑0 |
| `demo/utils/demo_logger.py` | 🔧 service | ↓9 ↑0 |
| `clients/python/src/proximadb_sdk/chunking_strategies/document_parsers.py` | 🔧 service | ↓4 ↑0 |

### Coupling Hotspots

- `clients/python/src/proximadb_sdk/models.py` - Many callers (↓14 ↑0)
- `clients/python/src/proximadb_sdk/chunking_strategies/code.py` - Calls many modules (↓3 ↑5)
- `clients/python/src/proximadb_sdk/unified_client_v2.py` - Many callers (↓14 ↑0)

## Important Notes

- Indexed 1928 files, 41248 symbols
- Check component paths above for exact file:line references
- Run `/init --update` to refresh after code changes
