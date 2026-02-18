# init.md

This file provides guidance to Victor when working with code in this repository.

## Project Overview

**proximaDB**: A high-performance vector database with support for graph analytics, embedding services, and scalable storage engines. Designed for large-scale semantic search and machine learning workflows, with a modular architecture enabling multi-language SDKs and extensible compute backends.

**Languages**: c (1), cpp (1), csharp (1), elixir (1), go (17), java (4), javascript (5), kotlin (1), php (1), python (429), ruby (1), rust (1828), scala (1), swift (1), typescript (26)

## Package Layout

| Path | Type | Description |
|------|------|-------------|
| `src/` | Active | Core system source code including storage engines, graph engines, API handlers, and observability components |
| `docs/` | Active | Documentation for architecture, design patterns, and usage guides |
| `examples/` | Active | Sample workflows demonstrating use cases including graph analytics, embedding pipelines, and vector search |
| `scripts/` | Active | Automation scripts for build, test, and deployment |
| `tests/` | Active | Unit and integration tests covering all major components |
| `demo/` | **ACTIVE** | End-to-end demos and tutorials with interactive examples |
| `clients/` | **ACTIVE** | SDKs and client libraries for Python, Node.js, and other languages |

## Key Components

| Component | Type | Path | Description |
|-----------|------|------|-------------|
| AIServiceState | struct | `src/api_handlers/ai_endpoints.rs:24` | Centralized state management for AI service operations, coordinating with `AutoMLService` and `BERTEmbeddingService` |
| AlertManager | struct | `src/storage/cache/health_monitor.rs:52` | Monitors storage health and triggers alerts based on `AlertingService` configurations |
| AlertingService | struct | `src/observability/alerting/mod.rs:25` | Centralized alerting system that integrates with Prometheus and custom metrics |
| ArtusBloomManager | struct | `src/storage/engines/impls/raptor/artus_bloom.rs:61` | Manages Bloom filters for fast membership testing in RAPTOR engine |
| AuthService | struct | `src/network/auth/mod.rs:127` | Implements authentication and authorization for secure API access |
| AutoMLService | struct | `src/automl/service.rs:167` | Handles automatic model selection and hyperparameter tuning for vector embeddings |
| BERTEmbeddingService | class | `demo/utils/bert_embedding_service.py:24` | Python-based service for generating BERT embeddings from text, integrated with `AutoMLService` |
| BackgroundMaintenanceManager | struct | `src/storage/persistence/write_ahead_log/background_manager.rs:53` | Manages background maintenance tasks like log rotation and compaction |
| BaseService | class | `clients/python/tests/chunking/resources/python/sample.py:30` | Base class for all services in Python SDK, providing common methods and error handling |

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
- `clients/nodejs-embedded/package-lock.json`
- `clients/nodejs-embedded/package.json`
- `clients/nodejs-embedded/tsconfig.esm.json`
- `clients/nodejs-embedded/tsconfig.json`
- `clients/python-embedded/pyproject.toml`
- `clients/python/pyproject.toml`
- `clients/python/tests/all_databases_results.json`
- `clients/python/tests/chunking/resources/json/sample.json`
- `clients/python/tests/chunking/resources/yaml/sample.yaml`
- `clients/python/tests/graph_benchmark_results.json`
- `clients/python/tests/reports/e2e/bert_flow_1759893259.json`

## Documentation

- `CLAUDE.md`
- `docs/SOLID_VECTOR_FRAMEWORK_REDESIGN.md`
- `demo/CONTRIBUTING.md`
- `clients/python/EMBEDDING_ARCHITECTURE_REDESIGN.md`
- `clients/python/GRAPH_API_TEST_RESULTS.md`
- `clients/python/EMBEDDING_PROVIDERS.md`
- `src/storage/engines/impls/sst/tests/SST_CORE_TESTS_ANALYSIS.md`
- `clients/python/examples/README.md`
- `demo/README.md`
- `demo/MIGRATION_GUIDE.md`

## Codebase Stats

- **1,153,753** lines of code across **2318** files
- Largest file: `src/proto/proximadb.v1.rs` (15,221 lines)
- Top files by size:
  - `src/proto/proximadb.v1.rs` (15,221 lines)
  - `src/storage/engines/core/formats/proximablocks/block_structures.rs` (6,607 lines)
  - `src/storage/engines/impls/sst/readers/sst_query_engine.rs` (6,399 lines)

## Analyzer Coverage

- **Symbol index**: 56903 symbols across 2318 files (multi-language tree-sitter + regex fallback, includes call sites and references)
- **Code graph**: not detected yet (run `victor index` to populate tree-sitter call/reference graph)
- **Semantic embeddings**: Tool Embeddings (6 files, 360.1 KB, 6h ago), Task Classifier (1 files, 1.5 MB, 6d ago), Conversation Embeddings (45 files, 11.9 MB, 4h ago) (tree-sitter chunking for code spans; tool/intent/conversation caches ready)
  - Code embeddings: 7345 vectors @ /Users/vijaysingh/code/proximaDB/.victor/embeddings (metadata keys: none)

## Embeddings & Chunking

- Code embeddings: 7345 vectors @ `/Users/vijaysingh/code/proximaDB/.victor/embeddings` (table: embeddings)

## Most Imported Modules

*Non-stdlib modules imported most frequently*

- `proximadb_sdk` (369 imports)
- `proximadb` (215 imports)
- `numpy` (149 imports)
- `google` (123 imports)
- `pytest` (105 imports)
- `grpc` (64 imports)
- `httpx` (44 imports)
- `embedding_utils` (42 imports)

## Named Implementations

### Graph Engines

| Name | Location | Description |
|------|----------|-------------|
| **ORION** | `src/graph/engines/orion/algorithms/centrality.rs:81` | senessCentrality implementation for graph centrality analysis |
| **PULSAR** | `src/graph/engines/pulsar/consensus/mod.rs:135` | consensus protocol for distributed graph state management |
| **QUASAR** | `src/graph/engines/quasar/mod.rs:108` | QuasarConfig for graph engine configuration |

### Storage Engines

| Name | Location | Description |
|------|----------|-------------|
| **HELIX** | `src/storage/engines/impls/helix/clustering.rs:21` | PCAModel for dimensionality reduction in clustering |
| **MIGRATION** | `src/storage/engines/migration/migrator.rs:18` | Migrator for handling data migrations between versions |
| **NOVA** | `src/storage/engines/impls/nova/batch_operations.rs:19` | BatchConfig for configuring batch operations |
| **ORION** | `src/graph/engines/orion/persistence.rs:41` | OrionSnapshot for point-in-time graph state persistence |
| **QUASAR** | `src/graph/engines/quasar/cache.rs:30` | AccessPatternCache for caching access patterns in graph queries |
| **RAPTOR** | `src/storage/engines/impls/raptor/adaptive_pxk.rs:33` | VectorSelection for adaptive vector selection in RAPTOR engine |
| **SST** | `src/storage/engines/impls/sst/block_format.rs:126` | BlockFormatWriter for writing SSTable blocks |
| **SWIFT** | `src/storage/engines/impls/swift/batch_operations.rs:17` | BatchConfig for Swift batch processing |
| **UNIVERSAL** | `src/storage/engines/universal/adapter.rs:33` | HardwareAccelerationManager for hardware-specific optimizations |
| **VIPER** | `src/storage/engines/impls/viper/codebook_sidecar.rs:18` | ViperCodebookSidecarManager for managing sidecar codebooks |

### Storage Ops

| Name | Location | Description |
|------|----------|-------------|
| **BASELINE** | `src/storage/engines/core/ops/proximacodec/impls/baseline/decoder.rs:15` | BaselineDecoder for standard decoding |
| **GPU** | `src/storage/engines/core/ops/proximacodec/impls/gpu/batching.rs:55` | GpuBatchSizer for GPU batch sizing |
| **SIMD** | `src/storage/engines/core/ops/proximacodec/impls/simd/decoder.rs:26` | SimdDecoder for SIMD-accelerated decoding |

### Storage Providers

| Name | Location | Description |
|------|----------|-------------|
| **LOCAL** | `clients/python/src/proximadb_sdk/embedding_providers/providers/local/bge.py:72` | BGE embedding provider for local embeddings |
| **TESTING** | `clients/python/src/proximadb_sdk/embedding_providers/providers/testing/simulated.py:34` | Simulated embedding provider for testing purposes |

## Performance Hints

*Extracted from docstrings and comments*

- `clients/python/examples/arrow_export_examples.py`: performance DataFrame library that works natively with Arrow,
- `clients/python/examples/embedding_providers_demo.py`: Performance Best for: Production retrieval, semantic search
- `clients/python/examples/sks_graph_first_demo.py`: Performance Comparison Summary Shows actual benchmarked performance, 20ms
- `clients/python/src/proximadb_sdk/__init__.py`: throughput operations: Large batch vector inserts (millions)
- `clients/python/src/proximadb_sdk/automl.py`: latency in milliseconds vector_count: Number of vectors, latency in milliseconds target_throughput: Target operations, latency
- `clients/python/src/proximadb_sdk/builders/collection.py`: performance collection for ML embeddings"
- `clients/python/src/proximadb_sdk/cache.py`: Performance metrics Usage: pool = ObjectPool( fac
- `clients/python/src/proximadb_sdk/chunking.py`: performance statistics, Performance monitoring and metrics via ResourcePool

## Architecture

1. **Component Pattern**: Found 55 component components, including `AuthService`, `AutoMLService`, and `AlertingService`
2. **Config Pattern**: Found 1335 config components, with configuration hierarchy from `.env` to CLI
3. **Controller Pattern**: Found 125 controller components, primarily in API handlers (`src/api_handlers/`)
4. **Factory Pattern**: Found 244 factory components, especially in `src/storage/engines/impls/`
5. **Middleware Pattern**: Found 358 middleware components, including auth and logging middleware in `src/network/`
6. **Model Pattern**: Found 511 model components, such as `AIServiceState`, `AlertManager`
7. **Observer Pattern**: Found 6 observer components, mainly in `src/observability/`
8. **Provider Pattern**: Found 462 provider components, including embedding providers in `clients/python/src/proximadb_sdk/embedding_providers/`

## Code Structure

- 1194 classes
- 2 components
- 1501 enums
- 39097 functions
- 5121 impls
- 72 interfaces
- 360 methods
- 2807 modules
- 6466 structs
- 256 traits
- 27 types

## Setup & Commands

```bash
npm install
pip install -e ".[dev]"
pytest
```

## Important Notes

- Indexed 2318 files, 56903 symbols
- Check component paths above for exact file:line references
- Run `/init --update` to refresh after code changes