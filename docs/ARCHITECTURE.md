# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

---

# ProximaDB Architecture Guide for AI Assistants

**Version**: 0.1.4
**Rust Edition**: 2024
**License**: Apache 2.0

This document provides a comprehensive architectural overview of ProximaDB, a cloud-native vector database with integrated graph capabilities, designed for AI assistants working on this codebase.

**Documentation Status**: Current as of November 2025 (partial verification)
- Most components reviewed against source code; known open WAL recovery issue noted below
- Test coverage counts approximate based on repository scan; see Verification Notes
- Hardware acceleration claims reflect feature-gated/experimental status where applicable

---

## Table of Contents

1. [Project Overview](#project-overview)
2. [Architectural Layers](#architectural-layers)
3. [Core Components](#core-components)
4. [Storage System](#storage-system)
5. [Query & Search](#query--search)
6. [Network & API Layer](#network--api-layer)
7. [Graph Database](#graph-database)
8. [Key Abstractions & Traits](#key-abstractions--traits)
9. [Build & Test Structure](#build--test-structure)
10. [Notable Features](#notable-features)
11. [Development Workflow](#development-workflow)
12. [Important Patterns](#important-patterns)

---

## Project Overview

ProximaDB is a **hybrid vector + graph database** built in Rust, optimized for:
- Semantic search and RAG (Retrieval-Augmented Generation) systems
- Knowledge graphs with vector embeddings
- High-throughput vector storage with type-safe metadata filtering
- Production workloads requiring persistence, consistency, and performance

### Key Characteristics

- **Single Binary Deployment**: Zero external dependencies
- **Proto-First Architecture**: Native protobuf flow, zero-copy operations
- **Multi-Engine Storage**: 6 specialized storage engines for different workloads
- **Dual Protocol**: REST (5678) and gRPC (5679) APIs running concurrently
- **Hardware Adaptive**: Automatic SIMD detection (AVX2/NEON), experimental GPU support
- **Cloud-Native**: S3/Azure/GCS support is feature-gated (`aws`, `azure`, `gcp`) and disabled by default; requires enabling features and credentials
- **Production-Scale**: ~536K LOC across ~1,043 Rust files (scan: `rg --files src`)
- **Well-Tested**: ~4,190 Rust tests detected (`#[test]` + `#[tokio::test]`; see Verification Notes)
- **Optional Surfaces**: AI dashboards, sales/trial flows, tenant access, and executive intelligence are kept behind Cargo features to keep the default surface minimal. See `docs/feature_toggles.md`.

---

## Architectural Layers

```text
┌─────────────────────────────────────────────────────────────┐
│                 Client Applications                          │
│              (Python SDK, REST, gRPC clients)                │
├─────────────────────────────────────────────────────────────┤
│                    API Layer                                 │
│         REST (Axum) + gRPC (Tonic) - Port 5678/5679         │
│              [network/rest, network/grpc]                    │
├─────────────────────────────────────────────────────────────┤
│                 Service Layer                                │
│   Collections | Operations | Search | Graph | Events        │
│           [services/collection, services/operations]         │
├─────────────────────────────────────────────────────────────┤
│        Index Layer              │      Compute Layer         │
│   AXIS (HNSW/IVF/LSH)          │   SIMD/GPU Acceleration    │
│   [index/axis]                  │   [compute/]               │
├─────────────────────────────────────────────────────────────┤
│                    Storage Layer                             │
│   WAL → MemTable → Storage Engines → Filesystem             │
│            [storage/engines, storage/persistence]            │
├─────────────────────────────────────────────────────────────┤
│              Persistence & Recovery                          │
│   Metadata Store | WAL System | Snapshot Management         │
│        [storage/metadata, storage/persistence/wal]           │
└─────────────────────────────────────────────────────────────┘
```

### Layer Responsibilities

1. **API Layer** (`src/network/`): Protocol handling, authentication, rate limiting
2. **Service Layer** (`src/services/`): Business logic, orchestration, transaction coordination
3. **Index Layer** (`src/index/`): Vector similarity search, adaptive index selection
4. **Compute Layer** (`src/compute/`): Hardware-accelerated vector operations
5. **Storage Layer** (`src/storage/`): Persistence, compaction, cache management
6. **Foundation** (`src/core/`): Types, errors, configuration, utilities

---

## Core Components

### 1. Main Entry Points

#### Server Binary (`src/bin/server.rs`)
- Production server with graceful shutdown
- Hardware capability detection
- Configuration loading and validation
- 6-stage startup sequence:
  1. Collections recovery from metadata
  2. Vectors recovery from WAL
  3. Graphs recovery from snapshots
  4. Assignments recovery
  5. Write buffer recovery
  6. HTTP/gRPC server startup

#### Library Root (`src/lib.rs`)
- `ProximaDB` struct: Main database instance
- Module organization and re-exports
- Proto-first architecture enforcement
- Cross-cache orchestrator initialization

### 2. Core Module (`src/core/`)

**Purpose**: Foundation types and utilities shared across all modules

**Key Modules**:
- `service_types.rs`: Core service types for vector operations (VectorRecord, Collection, etc.)
- `config.rs`: System configuration structures and defaults
- `config_loader.rs`: Configuration loading from files and environment
- `config_reloader.rs`: Dynamic configuration reloading
- `error.rs` & `errors/`: Unified error handling
- `hardware_capabilities.rs`: CPU/GPU feature detection
- `compression/`: Unified compression interface with adaptive selection
- `bloom/`: Probabilistic data structures for fast lookups
- `memory/`: Memory management, pools, and allocation strategies
- `quantization/`: This module has been moved to `src/compute/`.
- `serialization/`: Serialization utilities for various formats
- `resilience/`: Resilience patterns like Circuit Breaker and Retry
- `utils/`: Common utility functions

**Important Types**:
```rust
// Proto-first: VectorRecord is a direct alias to protobuf type
pub type VectorRecord = crate::proto::proximadb_v1::VectorRecord;

// Core service types
pub struct CollectionConfig { dimension, storage_engine, filterable_columns, ... }
pub struct SearchRequest { query_vector, top_k, filter_expression, ... }
pub enum DistanceMetric { Euclidean, Cosine, DotProduct, ... }
```

### 3. Configuration System (`src/core/config.rs`)

**Structure**:
```toml
[server]
bind_address = "0.0.0.0"
port = 5678
data_dir = "/data/proximadb"

[api]
rest_port = 5678
grpc_port = 5679
enable_tls = false

[storage]
default_engine = "sst"
storage_locations = [{ url = "file:///data", tier = "hot" }]
metadata_url = "file:///data/metadata"

[storage.wal]
enabled = true
sync_mode = "batch"
batch_size = 100

[cache]
total_memory_mb = 1024
eviction.enabled = true
```

**Config Files** (in `config/`):
- `config.toml`: Default configuration
- `production.toml`: Production settings
- `test-config.toml`: Test environment
- `minimal.toml`: Minimal setup

---

## Storage System

### Storage Engine Architecture

ProximaDB implements **6 specialized storage engines** using the Strategy Pattern:

```text
┌─────────────────────────────────────────────────────────┐
│            StorageEngineStrategy (Trait)                 │
├─────────────────────────────────────────────────────────┤
│  insert() | search() | flush() | compact()               │
└─────────────────────────────────────────────────────────┘
                           ▲
          ┌────────────────┼────────────────┐
          │                │                │
     ┌────▼───┐      ┌────▼───┐      ┌────▼───┐
     │   SST  │      │ VIPER  │      │  NOVA  │
     │ OLTP   │      │ OLAP   │      │ Hybrid │
     └────────┘      └────────┘      └────────┘
          │                │                │
     ┌────▼───┐      ┌────▼───┐      ┌────▼───┐
     │ SWIFT  │      │ RAPTOR │      │ HELIX  │
     │ Cache  │      │ Graph  │      │ PCA    │
     └────────┘      └────────┘      └────────┘
```

### Storage Engines (`src/storage/engines/impls/`)

#### 1. SST Engine (`sst/`)
**Best for**: OLTP, real-time queries, frequent updates

**Architecture**: LSM-tree with hybrid columnar format (ProximaBlocks)
- **Three-Stage Filtering**: Bloom filter -> Quantized vector filtering -> Full precision for maximum efficiency.
- **Compression**: LZ4 (fastest), Snappy, ZSTD.
- **Performance**: ~5.32ms for 10K vectors, <5ms for point lookups.
- **Write-Optimized**: LSM-tree architecture is optimized for frequent updates.
- **Zero-Copy Compaction**: For efficient background maintenance.

**Key Files**:
- `lib.rs`: Main SST implementation
- `flush/`: Memtable flushing logic
- `readers/`: Block readers with caching
- `search/`: Search implementation
- `unified_search_engine/`: Query execution

#### 2. VIPER Engine (`viper/`)
**Best for**: Analytics, batch operations, high-throughput workloads

**Architecture**: Columnar storage based on Apache Parquet, with Arrow integration.
- **Quantization Pipeline**: Multi-stage quantization (Binary -> INT8 -> PQ -> FP32) for optimal compression and performance.
- **Cloud-First Design**: Optimized for cloud storage with features like footer caching and range reads.
- **Analytics-Focused**: Efficient columnar storage is ideal for analytical queries and bulk operations.
- **Performance**: ~89.5ms for 10K vectors, with excellent write throughput.

**Key Files**:
- `lib.rs`: VIPER core
- `readers/`: Parquet file readers
- `write_engine.rs`: Batch writing

#### 3. NOVA Engine (`nova/`)
**Best for**: Complex analytical workloads, large-scale data mining

**Architecture**: Advanced columnar analytics engine with hierarchical optimization.
- **Hierarchical Statistics**: Multi-level SuperBlock metadata for intelligent query pruning (70-90% I/O reduction).
- **Advanced Zone Maps**: Multi-dimensional pruning beyond simple min/max filtering.
- **Streaming Architecture**: Memory-efficient processing of terabyte-scale data.
- **Cost-Based Optimization**: Intelligent query planning using data distribution statistics.
- **Progressive Search**: Adaptive query refinement with early termination.

#### 4. SWIFT Engine (`swift/`)
**Best for**: Hierarchical data, large-scale organized datasets

**Architecture**: Hierarchical storage with a three-tier architecture (SuperBlock -> DataBlock -> Records).
- **Hierarchical Indexing**: O(log n) access to data, ideal for organized datasets.
- **Large-Scale Support**: Optimized for datasets from millions to billions of vectors.
- **Use Cases**: Enterprise content management, multi-tenant systems, version control.
- **Performance**: ~94.1ms at 10K, with excellent query performance due to hierarchical pruning.

#### 5. RAPTOR Engine (`raptor/`)
**Best for**: Adaptive workload optimization, multi-tenant systems

**Architecture**: The innovative Matrix Trinity architecture (P²+K²+P×K matrices) for intelligent workload optimization.
- **Workload Adaptation**: Learns from query patterns to optimize performance in real-time.
- **Smart Resource Management**: Features adaptive row group sizing and memory-efficient operations.
- **Intelligent Compaction**: Uses pattern-aware compaction and consolidated reading.
- **Performance**: ~9.36ms for 10K vectors.

#### 6. HELIX Engine (`helix/`)
**Best for**: Spatially clustered data, image/video search, geospatial data

**Architecture**: Locality-optimized engine using PCA and Hilbert curves to cluster similar vectors.
- **Hilbert Curve Clustering**: Maps high-dimensional vectors to a 1D space, preserving locality for efficient range queries.
- **PCA Dimensionality Reduction**: Reduces the dimensionality of vectors before Hilbert mapping to improve clustering.
- **Spatial Pruning**: Achieves 90%+ query pruning by exploiting the spatial locality of the data.
- **Performance**: ~13.2ms for 10K vectors, with excellent performance on clustered data.

### Storage Formats (`src/storage/engines/core/formats/`)

#### ProximaBlocks (`proximablocks/`)
Custom block format for SST engine:
- Block header with metadata
- Compressed vector data
- Bloom filters per block
- Fast random access

#### Columnar (`columnar/`)
Arrow-based columnar storage:
- `parquet_write_engine/`: Parquet batch writing
- `columnar_query_engine/`: Analytics queries
- Schema evolution support

### Write-Ahead Log (`src/storage/persistence/write_ahead_log/`)

**Purpose**: Durability and crash recovery

**Implementation Scale**: 35 files, comprehensive production-ready WAL system

**Architecture**:
```text
Insert → WAL append → MemTable → Background Flush → Storage Engine
         (durable)    (fast)      (async)           (persistent)
```

**Key Components**:
- `mod.rs`: WriteAheadLogManager (118KB, core coordination)
- `manifest/`: Global WAL manifest system for multi-collection coordination
- `recovery_manager.rs`: Crash recovery (49KB, parallel recovery support)
- `flush_coordinator.rs`: Flush orchestration (31KB)
- `compaction_coordinator.rs`: WAL compaction (32KB)
- `disk_manager.rs`: Multi-disk support with load balancing (19KB)
- `serialization/`: Multiple formats (Proto 26KB, Avro 26KB, Bincode 28KB)
- `batch_strategy.rs`: Batch optimization (45KB)
- `parallel_recovery.rs`: Parallel crash recovery (14KB)

**Advanced Features**:
- Multiple serialization formats (Protocol Buffers, Avro, Bincode)
- Multi-disk support with RAID-like striping
- Atomic operations with MVCC
- Compression (LZ4, Snappy, ZSTD)
- Bloom filters for fast lookups
- Parallel recovery for faster crash recovery
- Collection affinity for locality
- TTL support for time-based expiry

**Resolved Issue (Previously "Known Issue")**:
- A previous version of this document described a "WAL pool metadata propagation bug" and suggested a fix involving a global `OnceLock` for the metadata provider.
- **WARNING**: Implementing this fix is known to cause a server hang on startup due to a deadlock. DO NOT USE a global metadata provider.
- **Resolution**: The bug was resolved by passing the metadata provider `Arc` through constructor parameters, not a global singleton. The WAL system now correctly resolves collection paths without the risk of deadlocks.

**Configuration**:
```rust
pub struct WALConfig {
    pub enabled: bool,
    pub sync_mode: SyncMode,  // Immediate, Batch, Async
    pub batch_size: usize,
    pub flush_interval_ms: u64,
    pub compression: CompressionAlgorithm,
    pub serialization_format: SerializationFormat,  // Proto, Avro, Bincode
}
```

### MemTable (`src/storage/memtable/`)

**Purpose**: In-memory write buffer with lock-free concurrency

**Implementations** (`implementations/`):
- `skiplist.rs`: Lock-free skip list (default)
- `btree.rs`: BTree-based memtable
- `art.rs`: Adaptive Radix Tree

**Features**:
- Concurrent writes with DashMap
- Automatic flushing at threshold
- WAL integration for durability
- Zero-copy reads with Arc

### Metadata Store (`src/storage/metadata/`)

**Purpose**: Collection metadata and schema management

**Backends** (`backends/`):
- `filesystem.rs`: File-based metadata (default)
- `rocksdb.rs`: RocksDB backend (optional)
- Cloud backends: S3, Azure, GCS

**Metadata Structure**:
```text
/metadata/
├── current/          # Active metadata
│   └── collections/
│       └── {uuid}.json
├── archive/          # Historical versions
└── __staging/        # Atomic writes
```

### Persistence Layer (`src/storage/persistence/`)

#### Filesystem Abstraction (`filesystem/`)
- `local.rs`: Local filesystem
- `s3.rs`: AWS S3 backend
- `azure.rs`: Azure Blob Storage
- `gcs.rs`: Google Cloud Storage
- `hdfs.rs`: Hadoop filesystem

#### Disk Manager (`disk_manager/`)
- Multi-disk support
- RAID-like striping
- Hot/warm/cold tiering

### Transaction Coordinator (`src/storage/transaction_coordinator.rs`)

**Purpose**: Atomic operations across WAL, MemTable, and Metadata

**Protocol**: Two-phase commit
1. Prepare: Write to staging area
2. Commit: Atomically move to active

**Lock-Free**: Uses DashMap for concurrent transactions

---

## Query & Search

### Query Module (`src/query/`)

**Purpose**: SQL and vector search query processing

#### SQL Frontend (`sql_frontend/`)
**SQL Support**:
```sql
-- Vector similarity search with filters
SELECT id, category, SIMILAR(embedding, ?) as score
FROM products
WHERE category = 'electronics' AND price < 1000
ORDER BY score DESC
LIMIT 10;

-- Graph traversal
SELECT id
FROM nodes
WHERE id IN (FOLLOW('start_node', 'edge_type', 2));
```

**Key Files**:
- `parser.rs`: SQL parsing with sqlparser crate and conversion to internal AST.
- `lowering.rs`: SQL → Internal AST
- `extensions.rs`: Vector function extensions

**Note:** The SQL frontend currently only supports `SELECT` queries. `CREATE COLLECTION` and other DDL statements are not supported via the SQL interface and must be performed using the REST or gRPC APIs.

#### Execution Engine (`execution/`)
- `executor.rs`: Query execution
- `pipeline.rs`: Execution pipeline
- `operators/`: Physical operators

#### Query Optimizer (`unified_query_optimizer/`)
**Cost-based optimization**:
- `cost_model.rs`: Cost estimation
- `statistics.rs`: Collection statistics
- `rules.rs`: Optimization rules
- `plan.rs`: Execution plans

**Optimization Rules**:
1. Filter selectivity analysis
2. Index selection (HNSW vs IVF vs Flat)
3. Predicate pushdown to storage
4. Join reordering
5. Parallel execution planning

#### Vector Search (`vector_search/`)
**Native vector search APIs**:
```rust
pub struct VectorSearchQuery {
    pub query_vector: Vec<f32>,
    pub top_k: usize,
    pub filter: Option<MetadataFilter>,
    pub distance_metric: DistanceMetric,
}
```

**Key Files**:
- `query.rs`: Search query structures
- `executor.rs`: Search execution
- `algorithms.rs`: Search algorithms (HNSW, IVF, etc.)

---

## Network & API Layer

### Network Module (`src/network/`)

**Architecture**: Dual protocol with unified handlers

```text
REST Client → Port 5678 → Axum HTTP → Unified Handlers → Services
gRPC Client → Port 5679 → Tonic     → Unified Handlers → Services
                              ↓
                      Middleware Stack
                   (Auth, Rate Limit, CORS, Metrics)
```

### REST API (`rest/`)

**Framework**: Axum (HTTP)

**Key Files**:
- `handlers/`: Request handlers
- `routes.rs`: Endpoint routing
- `v1/`: API v1 implementation

**Endpoints**:
```
POST   /api/v1/collections              # Create collection
GET    /api/v1/collections/{name}       # Get collection
POST   /api/v1/collections/{name}/vectors  # Insert vectors
POST   /api/v1/search                   # Vector search
GET    /health                          # Health check
GET    /metrics                         # Prometheus metrics
```

### gRPC API (`grpc/`)

**Framework**: Tonic (gRPC)

**Key Files**:
- `service.rs`: gRPC service implementation
- `streaming.rs`: Bidirectional streaming
- `interceptors.rs`: Request/response interceptors

**Services**:
```protobuf
service VectorDB {
    rpc CreateCollection(CreateCollectionRequest) returns (CollectionResponse);
    rpc InsertVectors(InsertVectorsRequest) returns (InsertResponse);
    rpc SearchVectors(SearchRequest) returns (SearchResponse);
    rpc StreamSearch(SearchRequest) returns (stream SearchResult);
}
```

### Protocol Buffers (`proto/proximadb/v1/`)

**Proto-First Design**: All data flows as protobuf types

**Key Protos**:
- `vector.proto`: Vector operations
- `collection.proto`: Collection management
- `graph.proto`: Graph operations
- `sql.proto`: SQL query support
- `types.proto`: Common types

### Middleware (`middleware/`)

**Cross-cutting concerns**:

1. **Authentication** (`auth.rs`)
   - JWT token validation
   - API key authentication
   - mTLS support

2. **Rate Limiting** (`rate_limit.rs`)
   - Per-client throttling
   - Token bucket algorithm
   - Configurable limits

3. **CORS** (`cors.rs`)
   - Cross-origin policies
   - Preflight handling

4. **Metrics** (`metrics.rs`)
   - Request latency tracking
   - Throughput monitoring
   - Error rate tracking

### Multi-Server (`multi_server.rs`)

**Purpose**: Concurrent REST + gRPC server orchestration

**Features**:
- Simultaneous server startup
- Graceful shutdown coordination
- Shared service layer
- Health monitoring

**Key Struct**:
```rust
pub struct MultiServer {
    config: MultiServerConfig,
    shared_services: Arc<SharedServices>,
    http_handle: Option<JoinHandle<()>>,
    grpc_handle: Option<JoinHandle<()>>,
}
```

---

## Graph Database

### Graph Module (`src/graph/`)

**Design Philosophy**: Proto-first, Arc-based zero-copy architecture

**Performance**:
- Traversal: 1M+ edges/second
- Node lookup: < 1μs
- Memory: < 100 bytes/node
- Arc clone: ~8 bytes (pointer copy)

### Graph Engines (`engines/`)

All three graph engines are fully implemented and production-ready:

#### 1. ORION Engine (`orion/`) - 7 files
**Best for**: In-memory graphs, real-time traversal, small to medium graphs (<1M nodes)

**Storage**: CSR (Compressed Sparse Row) format
- DashMap-based concurrent storage
- Arc zero-copy sharing (~8 bytes per clone)
- Cache-friendly adjacency layout
- Lock-free concurrent reads

**Performance**:
- Traversal: 1M+ edges/second
- Node lookup: <1μs (O(1) DashMap)
- Memory overhead: <100 bytes/node

**Features**:
- BFS/DFS traversal with max depth limits
- Property indexing (string, numeric, ordered)
- Label indexing for fast queries
- WAL persistence support (OrionPersistence) — note: pool WAL metadata propagation bug currently open (see Verification Notes)
- Unique constraints (single and multi-property)

#### 2. PULSAR Engine (`pulsar/`) - 5 files
**Best for**: Distributed graphs, horizontal scaling, fault tolerance (1B+ nodes)

**Architecture**: Distributed sharded storage
- Configurable shard count (default: 16 shards)
- Consistent hashing (SHA-256) for distribution
- 1-3x replication factor
- Consistency levels: Any, Quorum, All

**Features**:
- Each shard runs ORION engine internally
- Query coordinator for cross-shard operations
- Distributed BFS/DFS traversal
- Replication manager for fault tolerance
- Multi-datacenter deployment support

#### 3. QUASAR Engine (`quasar/`) - 4 files
**Best for**: Cost-optimized large graphs, sparse workloads, long-term retention

**Architecture**: Hybrid hot/cold tiering
- **Hot tier**: In-memory ORION engine (configurable max nodes/memory)
- **Cold tier**: Disk-based storage (SST, Parquet, or JSON backends)
- LRU-based cache management
- Access pattern tracking for automatic migration

**Features**:
- 80-90% storage cost reduction for large sparse graphs
- Automatic tier migration based on access patterns
- Background migration task (non-blocking)
- Transparent tier access (hot/cold abstraction)
- Tiering manager with configurable thresholds

### Graph Memory Pool (`mod.rs`)

**Arc-Based Sharing**:
```rust
pub struct GraphMemoryPool {
    pub nodes: Arc<DashMap<NodeId, Arc<Node>>>,
    pub edges: Arc<DashMap<EdgeId, Arc<Edge>>>,
    
    // Property indexes
    pub node_property_indexes: Arc<DashMap<String, DashMap<String, Vec<NodeId>>>>,
    pub edge_property_indexes: Arc<DashMap<String, DashMap<String, Vec<EdgeId>>>>,
    
    // Label indexes
    pub label_indexes: Arc<DashMap<String, Vec<NodeId>>>,
    pub edge_type_indexes: Arc<DashMap<String, Vec<EdgeId>>>,
}
```

**Zero-Copy Operations**:
- Arc::clone() for pointer sharing
- No data duplication
- Thread-safe reference counting

### Graph Service (`service.rs`)

**Business Logic Layer**:
- Node/edge CRUD operations
- Traversal execution
- Property indexing
- Constraint validation

**Key Methods**:
```rust
async fn create_node(&self, node: Node) -> Result<Arc<Node>>;
async fn create_edge(&self, edge: Edge) -> Result<Arc<Edge>>;
async fn traverse(&self, request: TraversalRequest) -> Result<TraversalResponse>;
async fn query_nodes(&self, query: NodeQuery) -> Result<Vec<Arc<Node>>>;
```

### Hybrid Query Engine (`hybrid/`)

**Purpose**: Combine vector similarity + graph traversal

**Example Query**:
```sql
-- Find similar products and their relationships
SELECT n.id, e.relationship_type, m.id
FROM nodes n
VECTOR_SEARCH embedding SIMILAR TO ? LIMIT 10
JOIN edges e ON e.from_node_id = n.id
JOIN nodes m ON e.to_node_id = m.id
WHERE m.category = 'complementary'
```

**Performance**: 2.14ms for hybrid queries (5,193 papers/sec)

---

## Key Abstractions & Traits

### Storage Traits (`src/storage/traits.rs`)

#### UnifiedStorageEngine
**Main storage abstraction**:
```rust
#[async_trait]
pub trait UnifiedStorageEngine: Send + Sync {
    async fn insert(&self, records: Vec<VectorRecord>) -> Result<InsertResult>;
    async fn search(&self, query: SearchQuery) -> Result<SearchResult>;
    async fn flush(&self, params: FlushParameters) -> Result<FlushResult>;
    async fn compact(&self, params: CompactionParameters) -> Result<CompactionResult>;
    async fn delete(&self, ids: Vec<VectorId>) -> Result<DeleteResult>;
    async fn health(&self) -> EngineHealth;
    async fn statistics(&self) -> EngineStatistics;
}
```

#### MetadataProvider
**Metadata operations abstraction**:
```rust
#[async_trait]
pub trait MetadataProvider: Send + Sync {
    async fn get_uuid(&self, collection_id: &str) -> Result<Option<String>>;
    async fn collection_metadata(&self, collection_id: &str) -> Result<Option<Collection>>;
    async fn list_collections(&self) -> Result<Vec<Collection>>;
}
```

### Compute Traits (`src/compute/distance_computation/`)

#### DistanceCompute
**Hardware-accelerated distance computation**:
```rust
pub trait DistanceCompute {
    fn compute_distances(
        &self,
        query: &[f32],
        database: &[Vec<f32>],
        metric: DistanceMetric,
    ) -> Vec<f32>;
    
    fn supported_metrics(&self) -> Vec<DistanceMetric>;
    fn backend(&self) -> ComputeBackend;
}
```

**Implementations**:
- `AVX2DistanceCompute`: ✅ AVX2 SIMD (8x f32 parallelism, implemented)
- `NEONDistanceCompute`: ✅ ARM NEON (4x f32 parallelism, implemented)
- `ScalarDistanceCompute`: ✅ Fallback (always available)
- `AVX512DistanceCompute`: ⚠️ Helper functions only (main kernels fall back to AVX2)
- `CUDADistanceCompute`: ⚠️ Feature-gated; code exists but requires PTX kernels and toolchain (not production-ready)
- `MetalDistanceCompute`: ⚠️ Detection only; kernels not implemented

### Index Traits (`src/index/axis/`)

#### AXIS Manager
**Adaptive eXperimental Index System**:
```rust
pub struct AxisManager {
    config: AxisConfig,
    index_type: IndexType,  // HNSW, IVF, LSH, etc.
    event_log: Arc<EventLog>,
}

impl AxisManager {
    pub async fn index_vector(&self, vector: VectorRecord) -> Result<()>;
    pub async fn search(&self, query: Vec<f32>, k: usize) -> Result<Vec<ScoredResult>>;
    pub async fn rebuild_index(&self) -> Result<()>;
}
```

**Index Types**:
- HNSW: Hierarchical Navigable Small World
- IVF: Inverted File Index
- LSH: Locality Sensitive Hashing
- PQ: Product Quantization
- Flat: Brute-force

---

## Build & Test Structure

### Build System

#### Cargo Workspace
**Single workspace** with multiple binaries:
```toml
[workspace]
members = ["."]

[[bin]]
name = "proximadb-server"
path = "src/bin/server.rs"

[[bin]]
name = "proximadb-bench"
path = "src/bin/proximadb-bench-consolidated.rs"
```

#### Build Script (`build.rs`)
**Responsibilities**:
1. Compile protobuf schemas with tonic-build
2. Generate descriptor sets for reflection
3. Compile CUDA kernels (if feature enabled)
4. Compile Metal shaders (macOS ARM64)
5. Add serde derives to proto enums

**GPU Support**:
- Linux x86_64: CUDA compilation
- macOS ARM64: Metal shader compilation
- Feature-gated: `--features gpu`

### Test Organization

**Test Coverage Statistics** (comprehensive testing):
- **Unit Tests**: ~80 tests (`#[test]` + `#[tokio::test]`)
- **Integration Tests**: ~174 test files in the `tests/` directory
- **Benchmarks**: 13 major benchmark suites + production validation

**Note:** The test counts are approximate and based on a simple `grep` and `find` commands. The actual number of tests may be higher due to test case macros and other dynamic test generation methods.

#### Unit Tests (`src/*/tests/`)
Co-located with source code:
- `src/storage/tests/`: Storage engine tests (SST, VIPER, NOVA, SWIFT, RAPTOR)
- `src/compute/tests/`: Distance computation, quantization, SIMD
- `src/graph/tests/`: Graph operations, traversal, engines
- `src/query/tests/`: SQL parsing, optimization, execution
- `src/network/tests/`: API endpoints, middleware

#### Integration Tests (`tests/`)
**Categories**:
- `integration/`: End-to-end API tests (REST, gRPC)
- `engines/`: Storage engine validation (all 6 engines)
- `graph/`: Graph database tests (8 comprehensive tests)
- `quantization/`: Multi-level quantization tests (13+ tests)
- `recovery/`: WAL and crash recovery tests
- `compression/`: Compression and codec tests
- `common/`: Shared test utilities (5 major helpers)
- `helpers/`: Test fixtures and builders

**Notable Test Suites**:
- `graph_integration_test.rs`: 8 tests (CRUD, traversal, concurrent ops)
- `sst_comprehensive_filter_test.rs`: Metadata filtering (all types)
- `raptor_integration_test.rs`: Matrix Trinity architecture (5 tests)
- `sks_graph_first_integration_test.rs`: Hybrid vector-graph queries
- `persistence_*.rs`: Multi-stage recovery validation
- `rest_api_operations.rs`: REST API endpoint testing (6 tests)

#### Benchmarks (`benches/`)
**Performance testing**:
- `bench_04_storage_unified.rs`: Storage engine comparison
- `bench_08_quantization_sst.rs`: Quantization performance
- `bench_10_query_progressive.rs`: Query optimization
- `bench_14_graph_operations.rs`: Graph performance
- `engine_performance_reporter.rs`: CSV reporting

**Running Benchmarks**:
```bash
make benchmark                    # All benchmarks
cargo bench --bench bench_04_storage_unified
cargo bench -- --save-baseline main
```

### Build Profiles

#### Development (`dev`)
```toml
[profile.dev]
opt-level = 0
debug = true
incremental = true
```

#### Release (`release`)
```toml
[profile.release]
opt-level = 3
lto = true
codegen-units = 1
```

#### Production (`release-server`)
```toml
[profile.release-server]
inherits = "release"
strip = true  # Remove debug symbols
```

### Makefile Commands

**Build**:
```bash
make build              # Debug build
make build-release      # Release build
make build-server       # Production build
```

**Test**:
```bash
make test               # All tests (Rust + Python)
make test-rust          # Rust tests only
make test-integration   # Integration tests
make test-python        # Python SDK tests
```

**Quality**:
```bash
make fmt                # Format code
make clippy             # Run clippy lints
make check              # Format + lint + test
```

**Server**:
```bash
make server-start       # Start debug server
make server-start-release  # Start release server

Authoritative development/test commands and coding standards are maintained in `AGENTS.md`; refer there for definitive guidance to keep assistants aligned.
```

---

## Notable Features

### 1. Proto-First Architecture

**Zero-Copy Design**:
```rust
// VectorRecord is a direct type alias to protobuf
pub type VectorRecord = crate::proto::proximadb_v1::VectorRecord;

// No conversions needed - proto flows end-to-end
async fn insert(request: InsertRequest) -> Result<InsertResponse> {
    let vectors: Vec<VectorRecord> = request.vectors;  // Direct use
    storage.insert(vectors).await  // No conversion
}
```

**Benefits**:
- No serialization overhead
- Single source of truth
- Direct field access
- No memory duplication

### 2. Hardware Acceleration

**CPU Detection** (`src/core/hardware_capabilities.rs`):
```rust
pub struct CpuFeatures {
    pub avx512: bool,
    pub avx2: bool,
    pub sse42: bool,
    pub neon: bool,  // ARM
}

pub fn detect_cpu_features() -> CpuFeatures {
    #[cfg(target_arch = "x86_64")]
    {
        // CPUID-based detection
        CpuFeatures {
            avx512: is_x86_feature_detected!("avx512f"),
            avx2: is_x86_feature_detected!("avx2"),
            sse42: is_x86_feature_detected!("sse4.2"),
            neon: false,
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        // ARM NEON always available on aarch64
        CpuFeatures {
            avx512: false,
            avx2: false,
            sse42: false,
            neon: true,
        }
    }
}
```

**GPU Backends** (experimental, infrastructure exists but not production-ready):
- CUDA: Detection + code exists (requires PTX kernel compilation)
- ROCm: Detection stubbed (not implemented)
- Metal: Device detection only (kernels return error)
- OpenCL: Detection + code exists (requires .cl kernel file)

### 3. Vector Compression via Quantization

**Primary Compression**: Multi-level quantization (`src/compute/quantization/`)
- **Binary (1-bit)**: 32x compression, 70-85% recall
- **INT8 (8-bit)**: 4x compression, 95%+ recall, with SIMD support (AVX2, NEON)
- **PQ4/PQ8**: Product Quantization, 8-16x compression
- **Adaptive**: Automatic level selection based on data characteristics

**Storage-Level Compression** (`src/storage/engines/`):
Storage engines use standard compression for on-disk data:
- **LZ4**: Default for SST engine (fast decompression, ~500 MB/s)
- **Snappy**: Alternative for balanced performance (~400 MB/s)
- **ZSTD**: VIPER engine default (best ratio, ~200 MB/s)
- Used for: WAL files, metadata, and storage blocks

**Unified Compression Interface** (`src/core/compression/`):
- Provides abstraction over multiple compression backends
- Adaptive selection based on data type and workload
- Streaming support for large datasets

### 4. Multi-Level Quantization

**Quantization Levels** (`src/compute/quantization/`):

1. **Binary (1-bit)**:
   - 32x compression
   - Fast initial filtering
   - 70-85% recall

2. **INT8 (8-bit)**:
   - 4x compression
   - 10x speedup
   - 95%+ recall

3. **PQ (Product Quantization)**:
   - 16x compression
   - Configurable subspaces
   - 90-95% recall

4. **Adaptive**:
   - Automatic selection
   - Data-dependent
   - Optimal accuracy/speed

**Usage**:
```rust
let engine = UnifiedQuantizationEngine::new();
let quantized = engine.quantize_int8(&vectors)?;
let distances = engine.compute_int8_distances(&query, &quantized)?;
```

### 5. Semantic Knowledge Store (SKS)

**Hybrid Architecture** (`src/storage/entity_store/`):

**Unified Entity Model**:
```rust
pub struct Entity {
    pub id: String,
    pub labels: Vec<String>,
    pub properties: HashMap<String, PropertyValue>,
    pub embedding: Option<Vec<f32>>,  // Vector component
    pub relationships: Vec<Edge>,      // Graph component
}
```

**Hybrid Queries**:
```rust
// Find similar entities with graph constraints
let results = sks.hybrid_search(
    vector_query,     // Vector similarity
    graph_pattern,    // Graph traversal
    filters           // Property filters
).await?;
```

**Performance**: 5,193 papers/sec, 2.14ms hybrid queries

### 6. Type-Safe Filtering

**Performance Impact**: -20% overhead (speeds up queries!)

**Filter DSL**:
```rust
pub struct FilterExpression {
    pub operator: LogicalOperator,  // AND, OR, NOT
    pub expressions: Vec<FieldCondition>,
}

pub struct FieldCondition {
    pub field: String,
    pub operator: ComparisonOperator,  // EQ, LT, GT, IN, etc.
    pub value: FilterValue,
}
```

**Validation**: 47 comprehensive filter tests

### 7. Persistence & Recovery

**6-Stage Recovery** (`src/lib.rs::ProximaDB::start()`):

1. **Collections**: Recover from metadata snapshots
2. **Vectors (WAL)**: Replay WAL files
3. **Graphs**: Load from snapshots + WAL
4. **Assignments**: Restore from metadata
5. **Buffers**: Recover write buffers
6. **Services**: Start HTTP/gRPC servers

**Automatic Persistence**:
- WAL enabled by default
- Graceful failure handling
- Zero configuration required

### 8. Cross-Cache Orchestrator

**Unified Caching** (`src/storage/cache/orchestrator/`):
```rust
pub struct CrossCacheOrchestrator {
    total_budget: usize,
    caches: Vec<Arc<dyn CacheBackend>>,
    eviction_service: Option<EvictionService>,
    warming_service: Option<WarmingService>,
}
```

**Features**:
- Memory budget management
- Automatic eviction
- Cache warming
- Rebalancing service

---

## Development Workflow

### Getting Started

1. **Prerequisites**:
   ```bash
   # Rust 1.88+ (2024 edition)
   rustup update stable
   
   # Build tools
   cargo install cargo-watch cargo-edit
   ```

2. **Clone and Build**:
   ```bash
   git clone https://github.com/vjsingh1984/proximaDB
   cd proximaDB
   make build
   ```

3. **Run Tests**:
   ```bash
   make test          # All tests
   make test-rust     # Rust only
   make test-python   # Python SDK
   ```

4. **Start Server**:
   ```bash
   make server-start  # Debug mode
   # Server runs on:
   # REST: http://localhost:5678
   # gRPC: http://localhost:5679
   ```

### Code Organization Principles

1. **Proto-First**: Always use protobuf types directly
2. **Lock-Free**: Prefer DashMap over RwLock
3. **Async-First**: Use async/await throughout
4. **Zero-Copy**: Use Arc for sharing, avoid cloning
5. **Error Handling**: Use anyhow::Result or thiserror

### Adding New Storage Engine

1. Create module: `src/storage/engines/impls/myengine/`
2. Implement `UnifiedStorageEngine` trait
3. Add to `StorageEngineStrategy` enum
4. Register in factory: `src/storage/engines/factory.rs`
5. Add tests: `tests/engines/myengine_test.rs`
6. Update benchmarks: `benches/bench_04_storage_unified.rs`

### Adding New API Endpoint

1. Define proto: `proto/proximadb/v1/myservice.proto`
2. Build: generates `src/proto/proximadb_v1.rs`
3. REST handler: `src/network/rest/handlers/myhandler.rs`
4. gRPC service: `src/network/grpc/myservice.rs`
5. Register routes: `src/network/rest/routes.rs`
6. Add tests: `tests/integration/api/myendpoint_test.rs`

### Performance Optimization

**Profiling**:
```bash
# CPU profiling
cargo flamegraph --bench bench_04_storage_unified

# Memory profiling
cargo instruments -t Allocations --bench bench_04_storage_unified

# Benchmarking with baseline
cargo bench -- --save-baseline main
# ... make changes ...
cargo bench -- --baseline main
```

**Common Optimizations**:
- Use `#[inline]` for hot paths
- Batch operations when possible
- Prefer stack allocation for small data
- Use streaming for large results
- Cache expensive computations

### Testing Guidelines

**Test Categories**:
1. Unit tests: Fast, isolated, deterministic
2. Integration tests: End-to-end API testing
3. Benchmarks: Performance regression detection
4. Fuzzing: Input validation (future)

**Test Structure**:
```rust
#[tokio::test]
async fn test_vector_insert() {
    // Arrange
    let db = setup_test_db().await;
    let vector = create_test_vector(128);
    
    // Act
    let result = db.insert(vector).await;
    
    // Assert
    assert!(result.is_ok());
    
    // Cleanup
    cleanup_test_db(db).await;
}
```

---

## Important Patterns

### 1. Error Handling

**Use anyhow for flexibility**:
```rust
pub async fn operation() -> anyhow::Result<Data> {
    let data = fetch_data()
        .await
        .context("Failed to fetch data")?;
    
    process_data(&data)
        .context("Failed to process data")?;
    
    Ok(data)
}
```

**Use thiserror for library errors**:
```rust
#[derive(thiserror::Error, Debug)]
pub enum StorageError {
    #[error("Collection not found: {0}")]
    NotFound(String),
    
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}
```

### 2. Lock-Free Concurrency

**Prefer DashMap over RwLock**:
```rust
// ❌ Avoid: Global lock
let map: Arc<RwLock<HashMap<K, V>>> = ...;

// ✅ Prefer: Lock-free sharding
let map: Arc<DashMap<K, V>> = ...;
```

**Arc for Zero-Copy Sharing**:
```rust
// ❌ Avoid: Cloning data
fn share_node(node: &Node) -> Node {
    node.clone()  // Full copy
}

// ✅ Prefer: Arc sharing
fn share_node(node: &Arc<Node>) -> Arc<Node> {
    Arc::clone(node)  // 8-byte pointer copy
}
```

### 3. Async Patterns

**Use tokio::spawn for background tasks**:
```rust
tokio::spawn(async move {
    loop {
        tokio::time::sleep(Duration::from_secs(60)).await;
        compact_data().await;
    }
});
```

**Use channels for communication**:
```rust
let (tx, rx) = tokio::sync::mpsc::channel(100);

// Producer
tokio::spawn(async move {
    tx.send(data).await.unwrap();
});

// Consumer
tokio::spawn(async move {
    while let Some(data) = rx.recv().await {
        process(data).await;
    }
});
```

### 4. Configuration Loading

**Layered configuration**:
```rust
// 1. Default config
let mut config = Config::default();

// 2. Load from file
config.merge_from_file("config.toml")?;

// 3. Override with CLI args
config.override_from_args(args)?;

// 4. Override with env vars
config.override_from_env()?;
```

### 5. Graceful Shutdown

**Signal handling**:
```rust
tokio::select! {
    _ = tokio::signal::ctrl_c() => {
        info!("Received SIGINT, shutting down...");
        db.stop().await?;
    }
    result = server.run() => {
        result?;
    }
}
```

**Cleanup order**:
1. Stop accepting new requests
2. Drain in-flight requests
3. Flush WAL and memtables
4. Close storage engines
5. Shutdown background tasks

---

## Performance Characteristics

### Storage Engine Performance (SIFT-1M Benchmark)

The following table shows the performance of ProximaDB's storage engines on the industry-standard SIFT-1M benchmark (100,000 vectors, 128 dimensions).

[cols="2,2,2,2,2a",options="header"]
|===
| Engine | Search Latency (ms) | Insert Throughput (vectors/s) | Recall@10 | Disk Usage (MB) | Notes

| SST
| ~79
| ~285,000
| 100%
| ~102
| Persistent, exact search

| HELIX
| ~78
| ~306,000
| 100%
| ~46
| Persistent, exact search

| VIPER
| ~79
| ~303,000
| 100%
| ~46
| Persistent, exact search

| NOVA
| ~78
| ~301,000
| 100%
| ~48
| Persistent, exact search

| SWIFT
| ~77
| ~300,000
| 100%
| ~51
| Persistent, exact search

| RAPTOR
| ~81
| ~280,000
| 100%
| ~87
| Persistent, exact search
|===

### Distance Computation (SIMD)

**SIMD-Optimized Metrics** (3 core metrics with full acceleration):

| Metric     | Scalar | AVX2 (x86_64) | NEON (ARM64) | Max Speedup |
|------------|--------|---------------|--------------|-------------|
| Euclidean  | 1x     | ~8x           | ~4x          | 8x          |
| Cosine     | 1x     | ~8x           | ~4x          | 8x          |
| Dot Product| 1x     | ~8x           | ~4x          | 8x          |

**Other Metrics** (14 total defined, scalar-only):
Manhattan, Hamming, Chebyshev, Minkowski, Jaccard, Canberra, Bray-Curtis, Angular, Hellinger, Custom, Unspecified

### Compression Performance

| Algorithm | Speed (MB/s) | Ratio | Decompress |
|-----------|--------------|-------|------------|
| LZ4       | 500+         | 2.0x  | 2000+ MB/s |
| Snappy    | 400+         | 2.2x  | 1500+ MB/s |
| ZSTD      | 200+         | 3.5x  | 800+ MB/s  |
| Gzip      | 50+          | 3.0x  | 200+ MB/s  |

### Memory Footprint

| Component       | Per Item | Notes                    |
|-----------------|----------|--------------------------|
| Vector (f32)    | 4B × dim | 768D = 3KB               |
| Graph Node      | 100B     | Includes properties      |
| Graph Edge      | 80B      | With relationship data   |
| Index (HNSW)    | 16B      | M=16, ef_construction=200|
| Metadata        | 256B     | JSON properties          |

---

## Configuration Reference

### Server Configuration

```toml
[server]
bind_address = "0.0.0.0"  # Listen address
port = 5678               # Primary port
node_id = "node1"         # Cluster node ID
data_dir = "/data"        # Data directory
```

### Storage Configuration

```toml
[storage]
default_engine = "sst"    # Default: SST, VIPER, NOVA, SWIFT, RAPTOR, HELIX
cache_size_mb = 1024      # Storage cache size
max_collections = 1000    # Maximum collections
compaction_threads = 2    # Background compaction threads

# Storage locations
[[storage.storage_locations]]
url = "file:///data/hot"
tier = "hot"              # hot, warm, cold, archive

[[storage.storage_locations]]
url = "s3://bucket/cold"
tier = "cold"

# Metadata storage
metadata_url = "file:///data/metadata"

# WAL configuration
[storage.wal]
enabled = true
sync_mode = "batch"       # immediate, batch, async
batch_size = 100
flush_interval_ms = 1000
compression = "lz4"

# SST engine specific
[storage.sst]
block_size_kb = 64
bloom_filter_bits = 10
compression = "lz4"

# VIPER engine specific
[storage.viper]
row_group_size = 10000
compression = "zstd"
compression_level = 3
```

### Cache Configuration

```toml
[cache]
total_memory_mb = 2048    # Total cache budget

[cache.eviction]
enabled = true
strategy = "lru"          # lru, lfu, arc
interval_seconds = 60

[cache.rebalancing]
enabled = true
interval_seconds = 300

enable_warming = false    # Cache warming (disabled by default)
```

### Network Configuration

```toml
[api]
rest_port = 5678
grpc_port = 5679
enable_tls = false
request_timeout_secs = 30
max_request_size = 67108864  # 64MB

[api.rate_limit]
enabled = true
requests_per_second = 1000
burst_size = 2000
```

### Hardware Configuration

```toml
[hardware]
auto_detect = true
preferred_backend = "avx2"  # avx512, avx2, neon, scalar

[hardware.cpu]
avx512 = true
avx2 = true
sse42 = true
neon = true

[hardware.gpu]
enabled = false
memory_limit_gb = 8.0
batch_size = 1024
```

---

## Troubleshooting

### Common Issues

1. **WAL Recovery Fails**
   ```
   Error: WAL recovery failed (continuing anyway)
   ```
   - Check WAL directory permissions
   - Verify disk space
   - Review WAL configuration

2. **Metadata Lock Timeout**
   ```
   Error: Metadata lock timeout
   ```
   - Reduce concurrent requests
   - Increase timeout in config
   - Check for deadlocks

3. **Memory Exhaustion**
   ```
   Error: Out of memory
   ```
   - Reduce cache size
   - Enable eviction
   - Lower batch sizes

### Debug Mode

**Enable verbose logging**:
```bash
RUST_LOG=debug cargo run --bin proximadb-server
```

**Log levels**:
- `error`: Errors only
- `warn`: Warnings and errors
- `info`: Informational messages (default)
- `debug`: Debug information
- `trace`: Very verbose

### Health Checks

**REST API**:
```bash
curl http://localhost:5678/health
```

**gRPC API**:
```bash
grpcurl -plaintext localhost:5679 health.v1.Health/Check
```

### Metrics

**Prometheus format**:
```bash
curl http://localhost:5678/metrics
```

**Key metrics**:
- `proximadb_requests_total`: Total requests
- `proximadb_request_duration_seconds`: Request latency
- `proximadb_storage_size_bytes`: Storage usage
- `proximadb_cache_hit_rate`: Cache effectiveness

---

## Future Roadmap

### Planned Features (2025)

1. **Distributed Clustering**
   - Multi-node deployment
   - Consistent hashing
   - Replication and failover

2. **Advanced Indexing**
   - Sparse vector support
   - Multi-vector search
   - Filtered HNSW

3. **Enhanced GPU Support**
   - CUDA kernel optimization
   - Multi-GPU support
   - Tensor core utilization

4. **AutoML Integration**
   - Automatic index tuning
   - Query optimization
   - Resource allocation

5. **Monitoring Dashboard**
   - Web UI for metrics
   - Query visualization
   - Performance profiling

---

## Resources

### Documentation

- **README**: `/README.adoc` - Project overview
- **API Docs**: `/docs/03-reference/rest-api-specification.adoc`
- **Performance**: `/docs/performance/README.adoc`
- **Architecture**: This file

### External Links

- **Repository**: https://github.com/vjsingh1984/proximaDB
- **Issue Tracker**: https://github.com/vjsingh1984/proximaDB/issues
- **Discussions**: https://github.com/vjsingh1984/proximaDB/discussions

### Community

- **Discord**: (Coming soon)
- **Slack**: (Coming soon)
- **Twitter**: @ProximaDB (Coming soon)

---

## License

Apache License 2.0

Copyright 2024-2025 Vijaykumar Singh

---

**Last Updated**: November 17, 2025
**Version**: 0.1.4
**Maintainer**: Vijaykumar Singh <singhvjd@gmail.com>

**Verification Notes**:
- Codebase scan: ~536K LOC across ~1,043 Rust files (`rg --files src | wc -l`; `wc -l`).
- Storage engines present for all six directories; WAL recovery currently affected by metadata propagation bug in pool managers (see Known Issue).
- Graph engines present (ORION, PULSAR, QUASAR); WAL issue may impact durability flows when persistence is enabled.
- Tests: ~4,190 Rust tests detected via grep of `#[test]`/`#[tokio::test]`; counts approximate and not stratified by unit/integration in this scan.
- Hardware acceleration: AVX2/NEON paths implemented; AVX-512/CUDA/Metal are experimental or build-time optional.
