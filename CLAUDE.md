# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Quick Start

### Docker (Fastest Way)
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
1. **Concrete Over Speculative**: Favor honest implementations over hype
2. **Reality-Based Development**: Build what exists, not what might exist
3. **No Simulation/Mocking**: Prefer real implementations over stubs
4. **Practical Over Perfect**: Choose working solutions over theoretical perfection
5. **Evidence-Based**: Base decisions on actual data, not assumptions

### Implementation Guidelines
- Write functioning code, not TODO comments
- Start with simplest working version
- Avoid elaborate abstractions until proven necessary
- Test against real data, not synthetic/mocked data
- Document what code actually does

### Documentation and Diagram Standards

#### Documentation Format Requirements
- **AsciiDoc Format**: All documentation must be in `.adoc` format, not Markdown
- **Consistent Structure**: Follow AsciiDoc best practices with proper heading hierarchy
- **Professional Presentation**: Use AsciiDoc features like callouts, admonitions, and structured blocks
- **Cross-References**: Use proper AsciiDoc cross-reference syntax for internal linking

#### Diagram and Visual Standards
- **Mermaid Diagrams Only**: Use Mermaid for all technical diagrams (architecture, flow, sequence, etc.)
- **Professional Styling**: All diagrams must follow the ProximaDB visual identity and logo style
- **Theme Compatibility**: Use colors that work in both light and dark browser themes
- **AsciiDoc Integration**: Mermaid diagrams must be embedded using `[source,mermaid]` blocks

##### Theme Compatibility Guidelines
1. **Use Neutral Theme**: Set `%%{init: {"theme": "neutral"}}%%` for best compatibility
2. **Avoid Pure White/Black**: Use `#000` for text on colored backgrounds, not `#ffffff`
3. **Medium Contrast Borders**: Use borders like `#2e5c8a`, `#5a5a5a` that show on both themes
4. **Readable Fill Colors**: Use fills like `#4a90e2`, `#5ba3f5` with sufficient opacity
5. **Test Both Themes**: Always preview diagrams in both light and dark mode
6. **Fallback to Simple**: When in doubt, use `theme: "neutral"` without custom variables

```asciidoc
[source,mermaid]
----
%%{init: {"theme": "base", "themeVariables": {"primaryColor": "#4a90e2", "primaryTextColor": "#000000", "primaryBorderColor": "#2e5c8a", "lineColor": "#5a5a5a", "sectionBkgColor": "#f0f4f8", "altSectionBkgColor": "#d6e4f0", "gridColor": "#b8b8b8", "tertiaryColor": "#fafafa"}}}%%
graph TB
    A[Component A] --> B[Component B]
    B --> C[Component C]

    style A fill:#4a90e2,stroke:#2e5c8a,stroke-width:2px,color:#000000
    style B fill:#5ba3f5,stroke:#3d7ab8,stroke-width:2px,color:#000000
    style C fill:#7db8f7,stroke:#5090d3,stroke-width:2px,color:#000000
----
```

#### Visual Style Guide
- **Primary Color**: `#4a90e2` (ProximaDB Blue - works in light/dark)
- **Secondary Colors**: `#5ba3f5`, `#7db8f7`, `#8fc4f9`
- **Accent Colors**: `#2e5c8a`, `#3d7ab8`, `#5090d3`
- **Background Colors**: `#f0f4f8`, `#d6e4f0`, `#fafafa`
- **Text Colors**: `#000000` (on colored fills), `#333333` (on light backgrounds)
- **Border Colors**: Medium tones that contrast with both light and dark backgrounds

#### Professional Icon Guidelines
**Approved Professional Symbols**:
- **Geometric Shapes**: Rectangles, circles, diamonds for components
- **Arrows**: Simple directional indicators (→, ←, ↑, ↓)
- **Professional Icons**: ▲ (priority), ● (status), ■ (component), ◆ (process)
- **System Symbols**: ⚡ (performance), ⚙ (configuration), 🔒 (security), 📊 (analytics)

**Avoid**: Casual emojis (😀, 🎉, 👍), decorative symbols (✨, 🌟), informal icons (📱, 💻, 🖥️)

**Professional Alternatives**:
- Instead of 📥/📤: Use "Input"/"Output" or simple arrows
- Instead of 🔄: Use "Process" or "Transform"
- Instead of 💾: Use "Storage" or "Database"
- Instead of ⚠️: Use "Error" or "Warning"

#### Common Diagram Templates

**Architecture Diagram**:
```asciidoc
[source,mermaid]
----
%%{init: {"theme": "neutral"}}}%%
graph TB
    subgraph CLIENT["CLIENT LAYER"]
        A[REST API<br/>Port 5678]
        B[gRPC API<br/>Port 5679]
    end
    subgraph SERVICE["SERVICE LAYER"]
        C[Collection Service]
        D[Vector Operations]
        E[Search Service]
    end
    subgraph STORAGE["STORAGE LAYER"]
        F[SST Engine]
        G[VIPER Engine]
        H[NOVA Engine]
    end
    A --> C
    B --> C
    C --> F
    D --> G
    E --> H
    style A fill:#4a90e2,stroke:#2e5c8a,stroke-width:2px,color:#000
    style B fill:#4a90e2,stroke:#2e5c8a,stroke-width:2px,color:#000
    style C fill:#5ba3f5,stroke:#3d7ab8,stroke-width:2px,color:#000
    style D fill:#5ba3f5,stroke:#3d7ab8,stroke-width:2px,color:#000
    style E fill:#5ba3f5,stroke:#3d7ab8,stroke-width:2px,color:#000
    style F fill:#7db8f7,stroke:#5090d3,stroke-width:2px,color:#000
    style G fill:#7db8f7,stroke:#5090d3,stroke-width:2px,color:#000
    style H fill:#7db8f7,stroke:#5090d3,stroke-width:2px,color:#000
    style CLIENT fill:#f0f4f8,stroke:#5a5a5a,stroke-width:1px,color:#000
    style SERVICE fill:#e8f0fa,stroke:#5a5a5a,stroke-width:1px,color:#000
    style STORAGE fill:#dde9f5,stroke:#5a5a5a,stroke-width:1px,color:#000
----
```

**Flow Diagram**:
```asciidoc
[source,mermaid]
----
%%{init: {"theme": "neutral"}}}%%
flowchart LR
    A[Input] --> B{Validation}
    B -->|Valid| C[Process]
    B -->|Invalid| D[Error Handler]
    C --> E[Storage]
    E --> F[Response]
    style A fill:#4a90e2,stroke:#2e5c8a,stroke-width:2px,color:#000
    style B fill:#ffd966,stroke:#cc9900,stroke-width:2px,color:#000
    style C fill:#5ba3f5,stroke:#3d7ab8,stroke-width:2px,color:#000
    style E fill:#7db8f7,stroke:#5090d3,stroke-width:2px,color:#000
    style F fill:#93c47d,stroke:#5a8047,stroke-width:2px,color:#000
    style D fill:#f4a261,stroke:#c8733d,stroke-width:2px,color:#000
----
```

**Sequence Diagram**:
```asciidoc
[source,mermaid]
----
%%{init: {"theme": "neutral"}}}%%
sequenceDiagram
    participant Client
    participant API
    participant Service
    participant Storage
    Client->>API: Request
    API->>Service: Validate & Process
    Service->>Storage: Store Data
    Storage-->>Service: Confirmation
    Service-->>API: Response
    API-->>Client: Result
----
```

## Architecture Overview

**Version**: 0.1.4 | **Rust Edition**: 2024 | **Min Rust**: 1.88

ProximaDB: Unified vector database with 6 specialized storage engines, AutoML capabilities, modularized architectures.

### Core Components
- **Storage Layer** (`src/storage/`): 6 engines implementing `UnifiedStorageEngine`
- **Compute Layer** (`src/compute/`): Unified quantization, hardware-accelerated distance computation
- **API Layer** (`src/api_handlers/`): REST (5678) + gRPC (5679)
- **Index Layer** (`src/index/`): AXIS engine (HNSW, IVF, PQ, etc.)
- **Services Layer** (`src/services/`): CollectionService, VectorOperationsService, EventLogService
- **AutoML Layer** (`src/automl/`): Automated optimization, workload prediction

### Storage Engines
- **SST**: Row-based, write-optimized, real-time queries (`src/storage/engines/impls/sst/`)
- **VIPER**: Columnar Parquet, analytics, batch ops (`src/storage/engines/impls/viper/`)
- **NOVA**: Progressive columnar, mixed workloads (`src/storage/engines/impls/nova/`)
- **SWIFT**: High-speed row-based, low-latency (`src/storage/engines/impls/swift/`)
- **RAPTOR**: Adaptive row-group, dynamic workloads (`src/storage/engines/impls/raptor/`)
- **HELIX**: Locality-optimized, Hilbert curve (`src/storage/engines/impls/helix/`)

### Key Design Patterns
- Proto-first pipeline with VectorRecord as native format
- Zero-copy operations throughout
- Unified quantization (`compute::quantization::unified`)
- Automatic hardware detection and optimization
- UnifiedCachingFilesystem for all storage backends

### Configuration
Primary file: `config/config.toml`
- `[server]`: Ports, data directories
- `[storage]`: Engine selection, storage locations, metadata URLs
- `[storage.write_buffer]`: Flush thresholds, memory limits
- `[storage.compaction]`: Background optimization
- `[compute.quantization]`: Compression algorithms

### Feature Flags
- `cloud-full`: All cloud storage backends
- `aws`, `azure`, `gcp`: Individual cloud backends
- `rocksdb`: RocksDB metadata backend
- `gpu`: GPU acceleration (CUDA, ROCm, MPS, OpenCL)

## Build and Development

### Essential Commands
```bash
# Build
cargo build                      # Debug
cargo build --release            # Release
cargo check --all-targets        # Fast check

# Test
cargo test --lib module_name     # Specific module
cargo test --no-fail-fast        # See all failures
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
- `proximadb-server`: Main server (src/bin/server.rs)
- `proximadb-bench`: Benchmarking (src/bin/proximadb-bench-consolidated.rs)
- `test_bloom_filter`, `test_engine_data_sizes`: Testing utilities
- `proximadb-bench-data-generator`: Test data generator

### Testing Strategy
- **Unit Tests**: In modules with `#[cfg(test)]`, `tests/unit/`
- **Integration Tests**: `tests/integration.rs`, `tests/*_integration_test.rs`
- **Engine Tests**: `tests/engines/`
- **Python SDK**: `tests/python/` (run with `PYTHONPATH=/workspace/clients/python/src python3 -m pytest -v`)
- **Benchmarks**: `benches/` (Criterion)

## Global WAL Manifest (v0.1.4)

Cloud-optimized global WAL manifest for multi-disk deployments:

**Configuration**:
```toml
[storage.wal_config]
global_manifest_url = "file:///path/to/manifest"  # Or s3://bucket/manifest
enable_wal = true
write_buffer_size_mb = 8192
```

**Architecture**:
- Location: `src/storage/persistence/write_ahead_log/manifest/`
- Centralized manifest tracks WAL segments across all disks
- Cloud-optimized append-only design (S3, Azure, GCS)
- LSN (Log Sequence Number) tracking
- Parallel recovery with thread pool
- Direct-to-storage recovery mode (faster startup)

**Manifest Entry**:
```rust
pub struct GlobalManifestEntry {
    pub lsn: u64,                    // Global LSN
    pub collection_id: String,       // Collection UUID
    pub storage_path: String,        // Disk location
    pub wal_segment_path: String,    // Relative path to WAL segment
    pub status: WalEntryStatus,      // Active, Flushed, Compacted, Deleted
    pub created_at: i64,
    pub size_bytes: u64,
}
```

**Recovery Flow**:
1. Load global manifest → discover WAL segments across disks
2. Parallel recovery (10 threads, CPU-based)
3. Direct-to-storage (bypasses memtable)
4. Progress tracking

**Debugging**:
```bash
# Enable WAL logging
RUST_LOG=info,proximadb::storage::persistence::write_ahead_log=debug \
  cargo run --bin proximadb-server

# View manifest
cat /tmp/proximadb/manifest/manifest_*.jsonl

# Monitor recovery
RUST_LOG=info cargo run --bin proximadb-server 2>&1 | grep -i "manifest\|global"
```

## Server Startup Sequence

1. **Hardware Detection** (15-20ms): CPU, SIMD, GPU, Memory
2. **SharedServices**: Metadata backend, collection service, cache orchestrator
3. **Global WAL Manifest**: Initialize, load segments, track LSN
4. **Storage Engine**: WAL recovery, load collections, start compaction workers
5. **Multi-Server**: gRPC (5679) + REST (5678)

**Complete Startup**: 100-200ms (empty DB), varies with data size

## Key Architecture Concepts

### Quantization System
`src/compute/quantization/unified.rs`:
- UnifiedQuantizationEngine: Hardware-accelerated, k-means++ clustering
- Codebook Storage: Persistent management via CodebookStore
- Levels: Binary, INT8, PQ4/8/16/32, automatic quality selection
- Hardware Acceleration: AVX2/AVX512/NEON SIMD

### Filesystem Architecture
`src/storage/persistence/filesystem/unified.rs`:
- UnifiedCachingFilesystem: Single entry point
- Integrated caching: metadata, disk, prefetch
- Zero-Copy I/O: ZeroCopyIOSystem
- Engine-aware metadata serialization

### Cache Architecture
`src/storage/cache/`:
- CacheOrchestrator: Central coordination
- VectorCache: Specialized for vector ops
- EvictionPolicy: LRU, LFU, Adaptive
- Integration: All engines use unified cache

## Common Development Patterns

### Adding New Storage Engine
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

Location: `clients/python/`

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

# Sequential tests (avoid race conditions)
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

**Collections found but no WAL entries** - Normal if no data inserted yet

**Diagnostic**:
```bash
ls -la /tmp/proximadb*/data/
cat /tmp/proximadb/manifest/manifest_*.jsonl | jq .
find /tmp/proximadb* -name "*.wal"
```

**Storage engine registration warning** - Expected during startup, collections load from metadata provider

**Manifest corruption**:
```bash
cp -r /tmp/proximadb/manifest /tmp/proximadb/manifest.backup
rm /tmp/proximadb/manifest/manifest_CORRUPTED.jsonl
RUST_LOG=info cargo run --bin proximadb-server
```

## 2025 Roadmap Implementation

Detailed guides: `docs/09-roadmap/implementation/`

**Q1**: Advanced Search Optimization, Quantized Vector Precomputation
**Q2**: Graph Database Enhancement, Multi-Modal Search
**Q3**: Advanced Security, Distributed Operations
**Q4**: AutoML Integration, LLM Support

Each guide includes: module organization, implementation, integration points, testing, config, migration path, metrics.

## Recent Changes (v0.1.4)

- Global WAL manifest with multi-disk coordination
- Cloud-optimized recovery (S3, Azure, GCS)
- LSN tracking for distributed systems
- Parallel recovery with thread pool
- Direct-to-storage recovery mode
- Multi-disk support with configurable storage
- Unified cache system implementation
- Enhanced logging with emoji-based status indicators
