# init.md

This file provides guidance to Victor when working with code in this repository.

## Project Overview

**proximaDB**: A high-performance, distributed database system optimized for vector similarity search, graph analytics, and machine learning embeddings. Built with Rust for core components and Python/TypeScript for client APIs and utilities.

**Languages**: javascript (4), python (328), rust (1275), typescript (13)

## Package Layout

| Path | Type | Description |
|------|------|-------------|
| `src/` | **ACTIVE** | Core Rust implementation including storage engines, graph engines, and AI services |
| `demo/` | **ACTIVE** | Example applications and demos using the library |
| `clients/` | **ACTIVE** | Client SDKs for Python, Node.js, and TypeScript |
| `tests/` | Active | Unit and integration tests |
| `docs/` | Active | Documentation and API references |

## Key Components

| Component | Type | Path | Description |
|-----------|------|------|-------------|
| AIServiceState | struct | `src/api_handlers/ai_endpoints.rs:22` | Manages AI inference state and session lifecycle |
| AlertManager | struct | `src/storage/cache/health_monitor.rs:52` | Monitors cache health and triggers alerts |
| AlertManager | struct | `src/index/axis/management/monitor.rs:71` | Index-level monitoring for performance degradation |
| ArtusBloomManager | struct | `src/storage/engines/impls/raptor/artus_bloom.rs:61` | Manages bloom filters for fast membership queries |
| AuthService | struct | `src/network/auth/mod.rs:127` | Handles authentication and authorization using JWT tokens |
| AutoMLService | struct | `src/automl/service.rs:167` | Implements automated model selection and tuning for vector operations |
| BERTEmbeddingService | class | `demo/utils/bert_embedding_service.py:24` | Service for generating BERT embeddings from text |
| BERTEmbeddingService | class | `clients/python/tests/integration/bert_embedding_service.py:24` | Integration test version of BERT embedding service |
| BERTEmbeddingService | class | `clients/python/tests/utils/bert_embedding_service.py:27` | Utility service for BERT embedding tests |
| BackgroundMaintenanceManager | struct | `src/storage/persistence/write_ahead_log/background_manager.rs:53` | Handles background log cleanup and compaction |
| BaseService | trait | `src/core/foundation/base_traits.rs:76` | Core trait defining common service behavior |
| CertificateManager | struct | `src/network/tls/certificate_manager.rs:61` | Manages TLS certificates for secure communication |

## Named Implementations

### Graph Engines

| Name | Path | Description |
|------|------|-------------|
| **ORION** | `src/graph/engines/orion/index.rs` | Property graph index with fast traversal capabilities |
| **PULSAR** | `src/graph/engines/pulsar/coordinator.rs` | Query coordinator for distributed graph queries |
| **QUASAR** | `src/graph/engines/quasar/mod.rs` | Optimized engine for high-throughput graph analytics |

### Storage Engines

| Name | Path | Description |
|------|------|-------------|
| **HELIX** | `src/storage/engines/impls/helix/clustering.rs` | PCA-based clustering model for vector dimensionality reduction |
| **MIGRATION** | `src/storage/engines/migration/migrator.rs` | Migrator for transitioning between engine versions |
| **NOVA** | `src/storage/engines/impls/nova/batch_operations.rs` | Batch configuration for optimized write operations |
| **ORION** | `src/graph/engines/orion/persistence.rs` | Orion snapshot manager for point-in-time recovery |
| **QUASAR** | `src/graph/engines/quasar/cache.rs` | Access pattern cache for query optimization |
| **RAPTOR** | `src/storage/engines/impls/raptor/adaptive_pxk.rs` | Vector selection engine with adaptive indexing |
| **SST** | `src/storage/engines/impls/sst/blocks.rs` | SST record block manager for efficient storage |
| **SWIFT** | `src/storage/engines/impls/swift/batch_operations.rs` | Batch configuration optimized for speed |
| **UNIVERSAL** | `src/storage/engines/universal/adapter.rs` | Hardware acceleration manager for GPU/CPU offloading |
| **VIPER** | `src/storage/engines/impls/viper/codebook_sidecar.rs` | Viper codebook sidecar manager for quantized vector storage |

### Storage Ops

| Name | Path | Description |
|------|------|-------------|
| **BASELINE** | `src/storage/engines/core/ops/proximacodec/impls/baseline/decoder.rs` | Baseline decoder for legacy compatibility |
| **GPU** | `src/storage/engines/core/ops/proximacodec/impls/gpu/batching.rs` | GPU batch sizer for parallel decoding |
| **SIMD** | `src/storage/engines/core/ops/proximacodec/impls/simd/decoder.rs` | SIMD decoder for vectorized operations |

### Storage Providers

| Name | Path | Description |
|------|------|-------------|
| **LOCAL** | `clients/python/src/proximadb/embedding_providers/providers/local/bge.py` | BGE embedding provider for local inference |
| **TESTING** | `clients/python/src/proximadb/embedding_providers/providers/testing/simulated.py` | Simulated embedding provider for testing |

**Key features:**
- Optimized for semantic search and production retrieval
- Support for distributed vector indexing
- GPU acceleration for embedding computation

## Performance Hints

*Extracted from docstrings and comments*

- `clients/python/examples/embedding_providers_demo.py`: Performance
Best for: Production retrieval, semantic search
- `clients/python/src/proximadb/builders/collection.py`: performance collection for ML embeddings")
- `clients/python/src/proximadb/cache.py`: Performance metrics

Usage:
    pool = ObjectPool(
        fac
- `clients/python/src/proximadb/chunking.py`: Performance monitoring and metrics via ResourcePool

Performan, performance statistics
- `clients/python/src/proximadb/embedding_providers/__init__new.py`: o("gte-qwen")
- `clients/python/src/proximadb/embedding_providers/core/config.py`: performance scores, and usage requirements
- `clients/python/src/proximadb/embedding_providers/core/registry.py`: o("gte-qwen")
- `clients/python/src/proximadb/embedding_providers/e5.py`: performance:
- Queries should have "query: " prefix
- Passages, performance on retrieval tasks, performance:
- Queries: "query: " prefix
- Passages: "passage:

## Architecture

1. **Component Pattern**: Found 23 component components
2. **Config Pattern**: Found 907 config components
3. **Controller Pattern**: Found 57 controller components
4. **Factory Pattern**: Found 126 factory components
5. **Middleware Pattern**: Found 205 middleware components
6. **Model Pattern**: Found 285 model components
7. **Provider Pattern**: Found 281 provider components
8. **Repository Pattern**: Found 372 repository components

## Code Structure

- 589 classes
- 948 enums
- 21236 functions
- 2905 impls
- 17 interfaces
- 2 methods
- 1802 modules
- 4169 structs
- 144 traits
- 3 types

## Common Commands

```bash
# Python project
pip install -e ".[dev]"
pytest
# Node.js project
npm install
npm test
# Rust project
cargo build
cargo test
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
- How does the graph engine integration work with the storage engines?

## Important Notes

- Indexed 1620 files, 31815 symbols
- Check component paths above for exact file:line references
- Run `/init --update` to refresh after code changes

## Component Relationships

### Core Architecture Relationships

- `AuthService` integrates with `CertificateManager` for secure token validation
- `AutoMLService` coordinates with `BackgroundMaintenanceManager` for periodic tuning
- `AIServiceState` is used by `AlertManager` for monitoring AI inference health
- `ArtusBloomManager` is part of `RAPTOR` engine's fast membership query system
- `BaseService` trait is implemented by all major service structs for consistency
- `Graph Engines` (`ORION`, `PULSAR`, `QUASAR`) interact with `Storage Engines` for data persistence
- `Storage Engines` (`HELIX`, `NOVA`, `RAPTOR`) leverage `Storage Ops` (`BASELINE`, `GPU`, `SIMD`) for vector decoding
- `Storage Providers` (`LOCAL`, `TESTING`) are used by `BERTEmbeddingService` for embedding generation

### Data Flow Patterns

- `AIServiceState` → `AlertManager` → `BackgroundMaintenanceManager` for monitoring and tuning
- `AutoMLService` → `Storage Engines` for model selection and optimization
- `BERTEmbeddingService` → `Storage Providers` for embedding generation and storage
- `Graph Engines` → `Storage Engines` for persistent graph data and metadata
- `Storage Engines` → `Storage Ops` for optimized vector operations
- `Storage Providers` → `Storage Engines` for embedding data ingestion

### Key Dependencies

- `AuthService` depends on `CertificateManager` for secure communication
- `AutoMLService` depends on `BackgroundMaintenanceManager` for periodic operations
- `AlertManager` depends on `AIServiceState` for inference monitoring
- `ArtusBloomManager` depends on `RAPTOR` engine for vector indexing
- `BaseService` is inherited by `AuthService`, `AutoMLService`, and `CertificateManager`
- `Storage Engines` depend on `Storage Ops` for decoding and encoding vectors
- `Storage Providers` depend on `BERTEmbeddingService` for embedding generation

### Performance Critical Paths

1. `BERTEmbeddingService` → `Storage Providers` → `Storage Engines` (embedding pipeline)
2. `Graph Engines` → `Storage Engines` → `Storage Ops` (query pipeline)
3. `AutoMLService` → `BackgroundMaintenanceManager` → `Storage Engines` (optimization pipeline)
4. `AIServiceState` → `AlertManager` → `BackgroundMaintenanceManager` (monitoring pipeline)