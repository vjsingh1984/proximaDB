# init.md

This file provides guidance to Victor when working with code in this repository.

## Project Overview

**proximaDB**: A multi-language, distributed vector database optimized for semantic search and retrieval. Supports embedding models, graph indexing, and scalable storage engines across languages including Rust, Python, and JavaScript. Built for high-performance ML workflows with rich tooling for data ingestion, chunking, and analytics.

**Languages**: c (1), cpp (1), csharp (1), elixir (1), go (2), java (4), javascript (5), kotlin (1), php (1), python (373), ruby (1), rust (1305), scala (1), swift (1), typescript (15)

## Package Layout

| Path | Type | Description |
|------|------|-------------|
| `src/` | Active | Source code for core engine, storage, and API components |
| `docs/` | Active | Documentation including architecture, API, and design decisions |
| `examples/` | Active | Examples and sample workflows for various use cases |
| `scripts/` | Active | Automation and helper scripts for CI, deployment, and devops |
| `tests/` | Active | Unit and integration tests covering core logic and workflows |
| `demo/` | **ACTIVE** | Source code for interactive demos and showcases |
| `clients/` | **ACTIVE** | SDKs and client libraries for multiple languages |

## Key Components

| Component | Type | Path | Description |
|-----------|------|------|-------------|
| AIServiceState | struct | `src/api_handlers/ai_endpoints.rs:24` | Manages state for AI inference services in API handlers |
| AlertManager | struct | `src/storage/cache/health_monitor.rs:52` | Monitors and reports on system health and cache status |
| ArtusBloomManager | struct | `src/storage/engines/impls/raptor/artus_bloom.rs:61` | Implements bloom filter logic for RAPTOR engine |
| AuthService | struct | `src/network/auth/mod.rs:127` | Handles authentication and authorization for API endpoints |
| AutoMLService | struct | `src/automl/service.rs:167` | Coordinates automated machine learning pipelines |
| BERTEmbeddingService | class | `demo/utils/bert_embedding_service.py:24` | Service for generating BERT embeddings from text |
| BackgroundMaintenanceManager | struct | `src/storage/persistence/write_ahead_log/background_manager.rs:53` | Manages background tasks for WAL and persistence |
| BaseService | class | `clients/python/tests/chunking/resources/python/sample.py:30` | Base class for all services. |

## Dependencies

**Core** (2 packages): @databricks/design-system, @openai/codex

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
- `clients/python/tests/chunking/resources/json/sample.json`
- `clients/python/tests/chunking/resources/yaml/sample.yaml`
- `clients/python/tests/reports/e2e/bert_flow_1759794976.json`
- `clients/python/tests/reports/e2e/bert_flow_1759799962.json`
- `clients/python/tests/reports/e2e/bert_flow_1759800282.json`
- `clients/python/tests/reports/e2e/bert_flow_1759875101.json`
- `clients/python/tests/reports/e2e/bert_flow_1759884518.json`
- `clients/python/tests/reports/e2e/bert_flow_1759893188.json`

## Documentation

- `docs/ARCHITECTURE.md`
- `PERSISTENCE_INFRASTRUCTURE_MAP.md`
- `DOCUMENTATION_INDEX.md`
- `ALL_ISSUES_COMPREHENSIVE_REVIEW.md`
- `DOCUMENTATION_STREAMLINING_ANALYSIS.md`
- `demo/CONTRIBUTING.md`
- `clients/python/EMBEDDING_ARCHITECTURE_REDESIGN.md`
- `clients/python/GRAPH_API_TEST_RESULTS.md`
- `clients/python/MIGRATION_V1_PROTO_SUMMARY.md`
- `clients/python/EMBEDDING_PROVIDERS.md`

## Codebase Stats

- **748,748** lines of code across **1713** files
- Largest file: `src/proto/proximadb.v1.rs` (7,899 lines)
- Top files by size:
  - `src/proto/proximadb.v1.rs` (7,899 lines)
  - `src/storage/engines/core/formats/proximablocks/block_structures.rs` (5,949 lines)
  - `src/bin/proximadb-bench-consolidated.rs` (5,469 lines)

## Analyzer Coverage

- **Symbol index**: 34897 symbols across 1713 files (multi-language tree-sitter + regex fallback, includes call sites and references)
- **Code graph**: 7175 nodes, 10717 edges (calls: 1332, refs: 1728, imports: 2336, inheritance: 171, composition: 1); module PageRank and coupling detection ready
  - Hub classes: `LanguageParser` (25 links), `BaseProximaDBTest` (23 links)
  - Module PageRank leaders: `clients/python/src/proximadb_sdk/models.py` (service), `clients/python/src/proximadb_sdk/chunking_strategies/code.py` (intermediary), `clients/python/tests/unit/conftest.py` (service)
  - Graph coverage by language: c (1), cpp (19), csharp (1)
  - Call hotspots: `get` (36 callers), `main` (20 callers), `create_collection` (17 callers)
- **Semantic embeddings**: Tool Embeddings (1 files, 69.8 KB, 1m ago), Task Classifier (1 files, 1.5 MB, 1m ago), Conversation Embeddings (11 files, 633.1 KB, 1m ago) (tree-sitter chunking for code spans; tool/intent/conversation caches ready)

## Graph Health

- Nodes: 7175, Edges: 10717 (CALLS: 1332, REFERENCES: 1728, IMPORTS: 2336, INHERITS: 171, COMPOSED_OF: 1)
- Language coverage: c (1), cpp (19), csharp (1), go (35)
- Hub classes: LanguageParser (25 links), BaseProximaDBTest (23 links), ProximaDBError (22 links)
- Module leaders: clients/python/src/proximadb_sdk/models.py (service), clients/python/src/proximadb_sdk/chunking_strategies/code.py (intermediary), clients/python/tests/unit/conftest.py (service)

## Most Imported Modules

*Non-stdlib modules imported most frequently*

- `proximadb_sdk` (362 imports)
- `proximadb` (170 imports)
- `numpy` (135 imports)
- `pytest` (98 imports)
- `google` (64 imports)
- `embedding_utils` (42 imports)
- `grpc` (42 imports)
- `httpx` (41 imports)

## Named Implementations

### Graph Engines

| Name | Location | Description |
|------|----------|-------------|
| **ORION** | `src/graph/engines/orion/index.rs:36` | PropertyIndex |
| **PULSAR** | `src/graph/engines/pulsar/coordinator.rs:36` | QueryCoordinator |
| **QUASAR** | `src/graph/engines/quasar/mod.rs:74` |   /// Engine |

### Storage Engines

| Name | Location | Description |
|------|----------|-------------|
| **HELIX** | `src/storage/engines/impls/helix/clustering.rs:21` | PCAModel |
| **MIGRATION** | `src/storage/engines/migration/migrator.rs:18` | Migrator {
    |
| **NOVA** | `src/storage/engines/impls/nova/batch_operations.rs:18` | BatchConfig |
| **ORION** | `src/graph/engines/orion/persistence.rs:41` | OrionSnapshot |
| **QUASAR** | `src/graph/engines/quasar/cache.rs:30` | AccessPatternCache |
| **RAPTOR** | `src/storage/engines/impls/raptor/adaptive_pxk.rs:33` | ectorSelection  |
| **SST** | `src/storage/engines/impls/sst/blocks.rs:37` | SstRecord |
| **SWIFT** | `src/storage/engines/impls/swift/batch_operations.rs:17` | BatchConfig |
| **UNIVERSAL** | `src/storage/engines/universal/adapter.rs:33` | HardwareAccelerationManager |
| **VIPER** | `src/storage/engines/impls/viper/codebook_sidecar.rs:19` | ViperCodebookSidecarManager |

### Storage Ops

| Name | Location | Description |
|------|----------|-------------|
| **BASELINE** | `src/storage/engines/core/ops/proximacodec/impls/baseline/decoder.rs:15` | BaselineDecoder |
| **GPU** | `src/storage/engines/core/ops/proximacodec/impls/gpu/batching.rs:55` | GpuBatchSizer |
| **SIMD** | `src/storage/engines/core/ops/proximacodec/impls/simd/decoder.rs:26` | SimdDecoder |

### Storage Providers

| Name | Location | Description |
|------|----------|-------------|
| **LOCAL** | `clients/python/src/proximadb_sdk/embedding_providers/providers/local/bge.py:73` | BGE embedding provider

**Key features:**
- Optimized for se |
| **TESTING** | `clients/python/src/proximadb_sdk/embedding_providers/providers/testing/simulated.py:34` | Simulated embedding provider for testing

Generates determin |

## Performance Hints

*Extracted from docstrings and comments*

- `clients/python/examples/embedding_providers_demo.py`: Performance
Best for: Production retrieval, semantic search
- `clients/python/examples/sks_graph_first_demo.py`: Performance Comparison Summary

Shows actual benchmarked perfo, 20ms
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

1. **Component Pattern**: Found 21 component components
2. **Config Pattern**: Found 978 config components
3. **Controller Pattern**: Found 67 controller components
4. **Factory Pattern**: Found 132 factory components
5. **Middleware Pattern**: Found 226 middleware components
6. **Model Pattern**: Found 292 model components
7. **Provider Pattern**: Found 302 provider components
8. **Repository Pattern**: Found 396 repository components

## Code Structure

- 993 classes
- 1 components
- 964 enums
- 23534 functions
- 2988 impls
- 30 interfaces
- 95 methods
- 1870 modules
- 4264 structs
- 150 traits
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

- What are the key components in the project? Summary modules one at a time along with one imrovement ...

## Code Graph Insights

*7175 symbols, 10717 relationships*

### Most Important Symbols (PageRank)

| Symbol | Type | Connections |
|--------|------|-------------|
| `get` (clients/python/tests/utils/test_helpers.py:66) | function | ↓36 ↑0 |
| `main` (demo/sks_demo.py:276) | function | ↓20 ↑0 |
| `create_collection` (demo/showcases/industry/ai_knowledge_base_demo.py:283) | function | ↓17 ↑0 |
| `__init__` (demo/setup.py:49) | function | ↓15 ↑0 |
| `insert_vectors` (clients/python/tests/unit/conftest.py:241) | function | ↓15 ↑0 |
| `delete_collection` (clients/python/tests/unit/conftest.py:173) | function | ↓13 ↑0 |

### Hub Classes (High Connectivity)

- `LanguageParser` (clients/python/src/proximadb_sdk/chunking_strategies/code.py:149) - 25 connections
- `BaseProximaDBTest` (clients/python/tests/utils/base_test.py:26) - 23 connections
- `ProximaDBError` (clients/python/src/proximadb_sdk/exceptions.py:22) - 22 connections

### Key Modules (Architecture)

| Module | Role | Connections |
|--------|------|-------------|
| `clients/python/src/proximadb_sdk/models.py` | 🔧 service | ↓14 ↑0 |
| `clients/python/src/proximadb_sdk/chunking_strategies/code.py` | ↔️ intermediary | ↓3 ↑4 |
| `clients/python/tests/unit/conftest.py` | 🔧 service | ↓13 ↑0 |
| `demo/utils/demo_logger.py` | 🔧 service | ↓13 ↑0 |
| `clients/python/src/proximadb_sdk/document_processor.py` | 🔧 service | ↓5 ↑0 |
| `clients/python/src/proximadb_sdk/chunking_strategies/parser_utils.py` | 🔧 service | ↓5 ↑0 |

### Coupling Hotspots

- `clients/python/src/proximadb_sdk/models.py` - Many callers (↓14 ↑0)
- `clients/python/tests/unit/conftest.py` - Many callers (↓13 ↑0)
- `demo/utils/demo_logger.py` - Many callers (↓13 ↑0)

## Important Notes

- Indexed 1713 files, 34897 symbols
- Check component paths above for exact file:line references
- Run `/init --update` to refresh after code changes