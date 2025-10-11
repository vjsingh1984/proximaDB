# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Quick Start

### Docker (Fastest)
```bash
docker run -d -p 5678:5678 -p 5679:5679 proximadb/proximadb:latest
curl http://localhost:5678/health
```

### From Source
```bash
git clone https://github.com/vjsingh1984/proximaDB
cd proximaDB
cargo run --release --bin proximadb-server
```

## Development Philosophy

### Core Principles
1. **Concrete Over Speculative**: Honest implementations over hype
2. **Reality-Based**: Build what exists, not what might exist
3. **No Mocking**: Real implementations over stubs
4. **Practical Over Perfect**: Working solutions over theory
5. **Evidence-Based**: Decisions based on data, not assumptions

### Implementation Guidelines
- Write functioning code, not TODO comments
- Start with simplest working version
- Avoid elaborate abstractions until proven necessary
- Test against real data, not synthetic data
- Document what code actually does

## Documentation Standards

### Format Requirements
- **AsciiDoc Only**: All documentation in `.adoc` format
- **Mermaid Diagrams**: All technical diagrams use Mermaid
- **AsciiDoc Integration**: Embed with `[source,mermaid]` blocks
- **Theme Compatible**: Use colors that work in light and dark modes

### Theme Compatibility
1. **Neutral Theme**: `%%{init: {"theme": "neutral"}}%%`
2. **Text Colors**: `#000` on colored backgrounds
3. **Borders**: Medium contrast (e.g., `#2e5c8a`, `#5a5a5a`)
4. **Fills**: Readable colors with sufficient opacity
5. **Test Both**: Preview in light and dark mode

### Visual Style Guide
**Colors**:
- Primary: `#4a90e2` (ProximaDB Blue)
- Secondary: `#5ba3f5`, `#7db8f7`, `#8fc4f9`
- Accent: `#2e5c8a`, `#3d7ab8`, `#5090d3`
- Background: `#f0f4f8`, `#d6e4f0`, `#fafafa`
- Text: `#000000` (on colored fills), `#333333` (on light)
- Border: Medium tones for both light/dark

**Professional Symbols**:
- Geometric: Rectangles, circles, diamonds
- Arrows: →, ←, ↑, ↓
- Professional: ▲ (priority), ● (status), ■ (component), ◆ (process)
- System: ⚡ (performance), ⚙ (config), 🔒 (security), 📊 (analytics)

**Avoid**: Casual emojis (😀, 🎉), decorative symbols (✨, 🌟), informal icons (📱, 💻)

### Diagram Templates

**Architecture**:
```asciidoc
[source,mermaid]
----
%%{init: {"theme": "neutral"}}%%
graph TB
    subgraph CLIENT["CLIENT LAYER"]
        A[REST API<br/>Port 5678]
        B[gRPC API<br/>Port 5679]
    end
    subgraph SERVICE["SERVICE LAYER"]
        C[Collection Service]
    end
    subgraph STORAGE["STORAGE LAYER"]
        D[Storage Engine]
    end
    A --> C
    B --> C
    C --> D
    style A fill:#4a90e2,stroke:#2e5c8a,stroke-width:2px,color:#000
    style B fill:#4a90e2,stroke:#2e5c8a,stroke-width:2px,color:#000
    style C fill:#5ba3f5,stroke:#3d7ab8,stroke-width:2px,color:#000
    style D fill:#7db8f7,stroke:#5090d3,stroke-width:2px,color:#000
----
```

**Flow**:
```asciidoc
[source,mermaid]
----
%%{init: {"theme": "neutral"}}%%
flowchart LR
    A[Input] --> B{Validation}
    B -->|Valid| C[Process]
    B -->|Invalid| D[Error]
    C --> E[Storage]
    style A fill:#4a90e2,stroke:#2e5c8a,stroke-width:2px,color:#000
    style B fill:#ffd966,stroke:#cc9900,stroke-width:2px,color:#000
    style C fill:#5ba3f5,stroke:#3d7ab8,stroke-width:2px,color:#000
    style D fill:#f4a261,stroke:#c8733d,stroke-width:2px,color:#000
    style E fill:#7db8f7,stroke:#5090d3,stroke-width:2px,color:#000
----
```

**Sequence**:
```asciidoc
[source,mermaid]
----
%%{init: {"theme": "neutral"}}%%
sequenceDiagram
    participant Client
    participant API
    participant Service
    participant Storage
    Client->>API: Request
    API->>Service: Process
    Service->>Storage: Store
    Storage-->>Service: Confirm
    Service-->>API: Response
    API-->>Client: Result
----
```

## Architecture Overview

**Version**: 0.1.4 | **Rust**: 2024 Edition | **Min Rust**: 1.88

Unified vector database with 6 specialized storage engines and AutoML capabilities.

### Core Components
- **Storage** (`src/storage/`): 6 engines implementing `UnifiedStorageEngine`
- **Compute** (`src/compute/`): Unified quantization, hardware-accelerated distance
- **API** (`src/api_handlers/`): REST (5678) + gRPC (5679)
- **Index** (`src/index/`): AXIS engine (HNSW, IVF, PQ)
- **Services** (`src/services/`): Collection, VectorOps, EventLog
- **AutoML** (`src/automl/`): Automated optimization, workload prediction

### Storage Engines
- **SST**: Hybrid columnar (ProximaBlocks), write-optimized, real-time (`src/storage/engines/impls/sst/`)
- **VIPER**: Columnar Parquet, analytics, batch (`src/storage/engines/impls/viper/`)
- **NOVA**: Progressive columnar, mixed workloads (`src/storage/engines/impls/nova/`)
- **SWIFT**: Hybrid columnar, ultra-low latency (`src/storage/engines/impls/swift/`)
- **RAPTOR**: Adaptive row-group, dynamic (`src/storage/engines/impls/raptor/`)
- **HELIX**: Hybrid columnar, locality-optimized, Hilbert curve (`src/storage/engines/impls/helix/`)

### Key Design Patterns
- Proto-first pipeline with VectorRecord
- Zero-copy operations
- Unified quantization (`compute::quantization::unified`)
- Automatic hardware detection
- UnifiedCachingFilesystem for all backends

### Configuration (`config/config.toml`)

**Primary Sections**:
```toml
[server]
node_id = "proximadb-local"        # Node identifier
bind_address = "127.0.0.1"         # Server bind address
port = 5678                        # Primary HTTP/REST port
data_dir = "/tmp/proximadb/data"   # Local data directory

[storage]
metadata_url = "file:///tmp/proximadb/metadata"  # Collection metadata storage

# Multiple storage locations for data distribution
[[storage.storage_locations]]
url = "file:///tmp/proximadb/d1"
weight = 1
tags = ["local", "disk1"]

[storage.wal_config]
global_manifest_url = "file:///tmp/proximadb/manifest"  # Global WAL manifest
memory_flush_size_bytes = 16777216      # 16MB per-collection flush threshold
global_flush_threshold = 4294967296     # 4GB total memory threshold
enable_wal = true
distribution_strategy = "LoadBalanced"   # WAL distribution across disks
collection_affinity = true               # Keep collection data together

[api]
grpc_port = 5679    # gRPC port
rest_port = 5678    # REST API port

[monitoring]
log_level = "info"
dashboard_refresh_interval_seconds = 60
```

**Optional Advanced Sections** (commented in config, defaults applied):
- `[storage.assignment_config]`: Storage location assignment strategy
- `[storage.sst_config]`: SST engine-specific settings
- `[storage.viper_config]`: VIPER engine-specific settings
- `[cache]`: Cache memory limits
- `[security]`: Security and authentication settings

### Feature Flags
- `cloud-full`: All cloud storage backends
- `aws`, `azure`, `gcp`: Individual cloud backends
- `rocksdb`: RocksDB metadata backend
- `gpu`: GPU acceleration (infrastructure exists, kernels present, but compilation disabled - uses CPU fallback, full support planned 2025)

## Build and Development

### Essential Commands
```bash
# Build
cargo build                      # Debug
cargo build --release            # Release
cargo check --all-targets        # Fast check

# Test
cargo test --lib module_name     # Specific module
cargo test --no-fail-fast        # All failures
RUST_LOG=debug cargo test name -- --nocapture  # Debug output

# Server
cargo run --bin proximadb-server
./target/release/proximadb-server --config config/config.toml

# Quality
cargo fmt                        # Format
cargo clippy -- -D warnings      # Lint
cargo bench                      # Benchmarks
cargo doc --open                 # Documentation
```

### Available Binaries
- `proximadb-server`: Main server (`src/bin/server.rs`)
- `proximadb-bench`: Benchmarking (`src/bin/proximadb-bench-consolidated.rs`)
- `proximadb-bench-data-generator`: Test data generator
- `test_bloom_filter`, `test_engine_data_sizes`: Testing utilities

### Testing Strategy
- **Unit**: In modules with `#[cfg(test)]`, `tests/unit/`
- **Integration**: `tests/integration.rs`, `tests/*_integration_test.rs`
- **Engines**: `tests/engines/`
- **Python**: `tests/python/` (run with `PYTHONPATH=/workspace/clients/python/src python3 -m pytest -v`)
- **Benchmarks**: `benches/` (Criterion)

## Global WAL Manifest (v0.1.4)

Cloud-optimized global WAL manifest for multi-disk deployments.

**Configuration**:
```toml
[storage.wal_config]
global_manifest_url = "file:///path/to/manifest"  # Or s3://bucket/manifest, azblob://container/manifest, gs://bucket/manifest
enable_wal = true
memory_flush_size_bytes = 16777216      # 16MB - per-collection flush threshold
global_flush_threshold = 4294967296     # 4GB - total memory threshold across all collections
distribution_strategy = "LoadBalanced"   # Options: LoadBalanced, RoundRobin, Random
collection_affinity = true               # Keep collection's WAL segments on same disk
```

**Architecture** (`src/storage/persistence/write_ahead_log/manifest/`):
- Centralized manifest tracks WAL segments across all disks
- Cloud-optimized append-only design (S3, Azure, GCS)
- LSN (Log Sequence Number) tracking
- Parallel recovery with thread pool
- Direct-to-storage recovery (faster startup)

**Manifest Entry**:
```rust
pub struct GlobalManifestEntry {
    pub lsn: u64,                    // Global LSN
    pub collection_id: String,       // Collection UUID
    pub storage_path: String,        // Disk location
    pub wal_segment_path: String,    // Relative path
    pub status: WalEntryStatus,      // Active, Flushed, Compacted, Deleted
    pub created_at: i64,
    pub size_bytes: u64,
}
```

**Recovery Flow**:
1. Load global manifest → discover WAL segments
2. Parallel recovery (10 threads, CPU-based)
3. Direct-to-storage (bypasses memtable)
4. Progress tracking

**Debugging**:
```bash
# Enable WAL logging
RUST_LOG=info,proximadb::storage::persistence::write_ahead_log=debug cargo run --bin proximadb-server

# View manifest
cat /tmp/proximadb/manifest/manifest_*.jsonl

# Monitor recovery
RUST_LOG=info cargo run --bin proximadb-server 2>&1 | grep -i "manifest\|global"
```

## Server Startup Sequence

1. **Hardware Detection** (15-20ms): CPU, SIMD, Memory
2. **SharedServices**: Metadata backend, collection service, cache orchestrator
3. **Global WAL Manifest**: Initialize, load segments, track LSN
4. **Storage Engine**: WAL recovery, load collections, start compaction
5. **Multi-Server**: gRPC (5679) + REST (5678)

**Complete Startup**: 100-200ms (empty DB), varies with data size

## Key Systems

### Quantization (`src/compute/quantization/unified.rs`)
- UnifiedQuantizationEngine: Hardware-accelerated, k-means++ clustering
- Codebook Storage: Persistent management via CodebookStore
- Levels: Binary, INT8, PQ4/8/16/32, automatic quality selection
- Hardware: AVX2/AVX512/NEON SIMD

### Filesystem (`src/storage/persistence/filesystem/unified.rs`)
- UnifiedCachingFilesystem: Single entry point
- Integrated caching: metadata, disk, prefetch
- Zero-Copy I/O: ZeroCopyIOSystem
- Engine-aware metadata serialization

### Cache (`src/storage/cache/`)
- CacheOrchestrator: Central coordination
- VectorCache: Specialized for vector ops
- EvictionPolicy: LRU, LFU, Adaptive
- Integration: All engines use unified cache

## Common Development Patterns

### Adding Storage Engine
1. Implement `UnifiedStorageEngine` in `src/storage/engines/impls/`
2. Use `compute::quantization::unified` for quantization
3. Use `UnifiedCachingFilesystem` for I/O
4. Add to factory in `src/storage/engines/factory.rs`
5. Add tests: module `#[cfg(test)]` + `tests/engines/`

### Modifying API Endpoints
1. Update `proto/proximadb.proto`
2. Run `cargo build` to regenerate proto types
3. Implement handlers in `src/api_handlers/`
4. Keep internal types separate from proto
5. Test with curl or Python client

### Key Files
- `src/storage/engines/factory.rs`: Engine selection
- `src/compute/quantization/unified.rs`: Unified quantization
- `src/storage/persistence/filesystem/unified.rs`: Unified filesystem
- `src/network/multi_server.rs`: REST + gRPC servers
- `proto/proximadb.proto`: Protocol definitions
- `config/config.toml`: Main configuration

## Python Client SDK

**Location**: `clients/python/`

**Installation**:
```bash
cd clients/python && pip install -e .
```

**Basic Usage**:
```python
from proximadb import ProximaDB

client = ProximaDB(url="http://localhost:5678")
collection = client.create_collection(name="my_collection", dimension=1536, engine="auto")
collection.insert([{"id": "vec_1", "vector": [0.1, 0.2, ...], "metadata": {"key": "value"}}])
results = collection.search(query_vector=[0.1, 0.2, ...], top_k=10, filter={"key": "value"})
```

**Tests**:
```bash
cd tests/python
PYTHONPATH=/workspace/clients/python/src python3 -m pytest -v
```

## Troubleshooting

### Common Issues
```bash
# Port conflicts
lsof -i :5678 && kill -9 $(lsof -t -i :5678)

# Permissions
chmod -R 755 ./data

# ARM64 build
cargo build --target aarch64-apple-darwin

# Clean rebuild
cargo clean && cargo build

# Sequential tests
cargo test -- --test-threads=1

# Clean test data
rm -rf /tmp/proximadb*
```

### Debugging
```bash
# Module-specific logging
RUST_LOG=proximadb::storage=trace cargo run --bin proximadb-server

# Backtrace
RUST_BACKTRACE=1 cargo run --bin proximadb-server

# Memory profiling
valgrind --leak-check=full cargo run --bin proximadb-server

# Thread debugging
RUST_LOG=tokio=trace cargo run --bin proximadb-server
```

### Recovery Issues

**Collections found but no WAL entries**: Normal if no data inserted yet

**Diagnostics**:
```bash
ls -la /tmp/proximadb*/data/
cat /tmp/proximadb/manifest/manifest_*.jsonl | jq .
find /tmp/proximadb* -name "*.wal"
```

**Storage engine registration warning**: Expected during startup, collections load from metadata provider

**Manifest corruption**:
```bash
cp -r /tmp/proximadb/manifest /tmp/proximadb/manifest.backup
rm /tmp/proximadb/manifest/manifest_CORRUPTED.jsonl
RUST_LOG=info cargo run --bin proximadb-server
```

## 2025 Roadmap

**Detailed guides**: `docs/09-roadmap/implementation/`

- **Q1**: Advanced Search Optimization, Quantized Vector Precomputation
- **Q2**: Graph Database Enhancement, Multi-Modal Search
- **Q3**: Advanced Security, Distributed Operations
- **Q4**: AutoML Integration, LLM Support

Each guide includes: module organization, implementation, integration, testing, config, migration, metrics.

## Recent Changes (v0.1.4)

- Global WAL manifest with multi-disk coordination
- Cloud-optimized recovery (S3, Azure, GCS)
- LSN tracking for distributed systems
- Parallel recovery with thread pool
- Direct-to-storage recovery mode
- Multi-disk support with configurable storage
- Unified cache system implementation
- Enhanced logging with emoji-based status indicators
