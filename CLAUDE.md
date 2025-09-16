# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Development Philosophy and Rules

### Core Principles
1. **Concrete Over Speculative**: Always favor honest, concrete implementations over hype or speculative features
2. **Reality-Based Development**: Build what actually exists in the codebase, not what might exist in the future
3. **No Simulation/Mocking**: Prefer real implementations over simulations, stubs, or placeholder code
4. **Practical Over Perfect**: Choose working solutions over theoretical perfection
5. **Evidence-Based**: Base architectural decisions on actual usage patterns and performance data, not assumptions

### Implementation Guidelines
- Write real, functioning code rather than TODO comments or placeholder functions
- When implementing features, start with the simplest working version that meets requirements
- Avoid creating elaborate abstractions until they're proven necessary by real use cases
- Test against actual data and real-world scenarios, not synthetic/mocked data
- Document what the code actually does, not what it's intended to do
- Remove or replace any placeholder/simulation code with concrete implementations

### Testing Requirements (TDD Approach)
- **50% Test Coverage Minimum**: Every code item being implemented must have at least 50% test coverage
- **Test-First Development**: Write tests before or alongside implementation, not as an afterthought
- **Unit Tests Required**: Each new function, struct, or module must have corresponding unit tests
- **Integration Tests**: New features must include integration tests that verify end-to-end functionality
- **Real Data Testing**: Tests should use realistic data patterns, not just minimal examples
- **Error Path Testing**: Test both success and failure scenarios for robust error handling
- **Performance Testing**: Include benchmark tests for performance-critical components
- **Test Organization**:
  - Unit tests: Inline `#[cfg(test)]` modules or `tests/` directory
  - Integration tests: `tests/` directory with `--test integration` flag
  - Benchmarks: `benches/` directory with `cargo bench`

### Documentation and Diagram Standards

#### Documentation Format Requirements
- **AsciiDoc Format**: All documentation must be in `.adoc` format, not Markdown
- **Consistent Structure**: Follow AsciiDoc best practices with proper heading hierarchy
- **Professional Presentation**: Use AsciiDoc features like callouts, admonitions, and structured blocks
- **Cross-References**: Use proper AsciiDoc cross-reference syntax for internal linking

#### Diagram and Visual Standards
- **Mermaid Diagrams Only**: Use Mermaid for all technical diagrams (architecture, flow, sequence, etc.)
- **Professional Styling**: All diagrams must follow the ProximaDB visual identity and logo style
- **AsciiDoc Integration**: Mermaid diagrams must be embedded using `[source,mermaid]` blocks:

```asciidoc
[source,mermaid]
----
%%{init: {"theme": "base", "themeVariables": {"primaryColor": "#1e88e5", "primaryTextColor": "#ffffff", "primaryBorderColor": "#0d47a1", "lineColor": "#1976d2", "sectionBkgColor": "#e3f2fd", "altSectionBkgColor": "#bbdefb", "gridColor": "#90caf9", "tertiaryColor": "#f5f5f5"}}}%%
graph TB
    A[Component A] --> B[Component B]
    B --> C[Component C]

    style A fill:#1e88e5,stroke:#0d47a1,stroke-width:2px,color:#ffffff
    style B fill:#2196f3,stroke:#1565c0,stroke-width:2px,color:#ffffff
    style C fill:#42a5f5,stroke:#1976d2,stroke-width:2px,color:#ffffff
----
```

#### Visual Style Guide
- **Primary Color**: `#1e88e5` (ProximaDB Blue)
- **Secondary Colors**: `#2196f3`, `#42a5f5`, `#64b5f6`
- **Accent Colors**: `#0d47a1`, `#1565c0`, `#1976d2`
- **Background Colors**: `#e3f2fd`, `#bbdefb`, `#f5f5f5`
- **Text Colors**: `#ffffff` (on colored backgrounds), `#333333` (on light backgrounds)
- **Professional Appearance**: Clean lines, consistent spacing, clear hierarchy
- **Icon Standards**: Use minimal, professional icons and geometric shapes
- **Typography**: Clear, readable labels without excessive decorative elements

#### Professional Icon Guidelines
**Approved Professional Symbols**:
- **Geometric Shapes**: Rectangles, circles, diamonds for components
- **Arrows**: Simple directional indicators (→, ←, ↑, ↓)
- **Professional Icons**: ▲ (priority), ● (status), ■ (component), ◆ (process)
- **System Symbols**: ⚡ (performance), ⚙ (configuration), 🔒 (security), 📊 (analytics)

**Avoid These Elements**:
- Casual emojis (😀, 🎉, 👍, etc.)
- Decorative symbols (✨, 🌟, 🎨, etc.)
- Informal icons (📱, 💻, 🖥️, etc.)
- Entertainment symbols (🎮, 🎵, 🎬, etc.)

**Professional Alternatives**:
- Instead of 📥/📤: Use "Input"/"Output" or simple arrows
- Instead of 🔄: Use "Process" or "Transform"
- Instead of 💾: Use "Storage" or "Database"
- Instead of ⚠️: Use "Error" or "Warning"

#### Common Diagram Templates

**Architecture Diagram Template**:
```asciidoc
[source,mermaid]
----
%%{init: {"theme": "base", "themeVariables": {"primaryColor": "#1e88e5", "primaryTextColor": "#ffffff", "primaryBorderColor": "#0d47a1", "lineColor": "#1976d2", "sectionBkgColor": "#e3f2fd", "altSectionBkgColor": "#bbdefb", "gridColor": "#90caf9", "tertiaryColor": "#f5f5f5"}}}%%
graph TB
    subgraph "CLIENT LAYER"
        A[REST API<br/>Port 5678]
        B[gRPC API<br/>Port 5679]
    end

    subgraph "SERVICE LAYER"
        C[Collection Service]
        D[Vector Operations]
        E[Search Service]
    end

    subgraph "STORAGE LAYER"
        F[SST Engine]
        G[VIPER Engine]
        H[NOVA Engine]
    end

    A --> C
    B --> C
    C --> F
    D --> G
    E --> H

    style A fill:#1e88e5,stroke:#0d47a1,stroke-width:2px,color:#ffffff
    style B fill:#1e88e5,stroke:#0d47a1,stroke-width:2px,color:#ffffff
    style C fill:#2196f3,stroke:#1565c0,stroke-width:2px,color:#ffffff
    style D fill:#2196f3,stroke:#1565c0,stroke-width:2px,color:#ffffff
    style E fill:#2196f3,stroke:#1565c0,stroke-width:2px,color:#ffffff
    style F fill:#42a5f5,stroke:#1976d2,stroke-width:2px,color:#ffffff
    style G fill:#42a5f5,stroke:#1976d2,stroke-width:2px,color:#ffffff
    style H fill:#42a5f5,stroke:#1976d2,stroke-width:2px,color:#ffffff
----
```

**Flow Diagram Template**:
```asciidoc
[source,mermaid]
----
%%{init: {"theme": "base", "themeVariables": {"primaryColor": "#1e88e5", "primaryTextColor": "#ffffff", "primaryBorderColor": "#0d47a1", "lineColor": "#1976d2"}}}%%
flowchart LR
    A[Input] --> B{Validation}
    B -->|Valid| C[Process]
    B -->|Invalid| D[Error Handler]
    C --> E[Storage]
    E --> F[Response]

    style A fill:#1e88e5,stroke:#0d47a1,stroke-width:2px,color:#ffffff
    style B fill:#1565c0,stroke:#0d47a1,stroke-width:2px,color:#ffffff
    style C fill:#2196f3,stroke:#1565c0,stroke-width:2px,color:#ffffff
    style E fill:#42a5f5,stroke:#1976d2,stroke-width:2px,color:#ffffff
    style F fill:#4caf50,stroke:#2e7d32,stroke-width:2px,color:#ffffff
    style D fill:#ff5722,stroke:#d84315,stroke-width:2px,color:#ffffff
----
```

**Sequence Diagram Template**:
```asciidoc
[source,mermaid]
----
%%{init: {"theme": "base", "themeVariables": {"primaryColor": "#1e88e5", "primaryTextColor": "#ffffff", "primaryBorderColor": "#0d47a1", "lineColor": "#1976d2", "actorBkg": "#e3f2fd", "actorBorder": "#1976d2", "actorTextColor": "#0d47a1", "activationBkgColor": "#bbdefb", "activationBorderColor": "#1976d2"}}}%%
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

## Build and Development Commands

### Building the Project
```bash
# Debug build
make build
cargo build

# Release build (production optimization)
make build-release
cargo build --release

# Optimized server build
make build-server
cargo build --profile release-server

# Full release build with tests and benchmarks
make release

# Clean build artifacts
make clean
cargo clean

# Check for compilation errors specifically
cargo build 2>&1 | tee current_error.log
```

### Available Binaries
- `proximadb-server`: Main database server (src/bin/server.rs)
- `proximadb-bench`: Benchmarking tool (src/bin/proximadb-bench-consolidated.rs)

### Running Tests
```bash
# Run all tests (Rust + Python)
make test

# Rust tests only
make test-rust
cargo test --verbose

# Integration tests
make test-integration
cargo test --test integration --verbose

# Python SDK tests
make test-python
cd clients/python && pytest tests/ -v

# Python integration tests (from tests/python directory)
cd tests/python && PROTOCOL_BUFFERS_PYTHON_IMPLEMENTATION=python PYTHONPATH=/workspace/clients/python/src python3 -m pytest -v

# Performance tests with server
make perf-test

# Full integration tests with real server
make integration-full

# Run specific test category
cargo test --test integration storage::

# Single test with debug output
RUST_LOG=debug cargo test test_name -- --nocapture

# Check current compilation status
cargo build 2>&1 | tee current_error.log
cat current_error.log | head -20  # Review recent errors
```

### Running the Server
```bash
# Development mode
make server-start
cargo run --bin proximadb-server

# Release mode (optimized)
make server-start-release
cargo run --release --bin proximadb-server

# Production mode with config
./target/release/proximadb-server --config config/config.toml

# With specific logging
RUST_LOG=proximadb=debug cargo run --bin proximadb-server

# Docker deployment (recommended for production)
make docker-build
make docker-run
docker run -d -p 5678:5678 -p 5679:5679 -v proximadb_data:/data proximadb/proximadb:latest
```

### Code Quality
```bash
# Format code
make fmt
cargo fmt

# Lint with clippy
make clippy
cargo clippy -- -D warnings

# Full quality check (format + lint + test)
make check

# Run benchmarks
make benchmark
cargo bench

# Specific benchmarks
make benchmark-vector
make benchmark-metadata
cargo bench --bench vector_operations
cargo bench --bench metadata_lifecycle

# Run benchmark binary
cargo run --bin proximadb-bench

# Generate documentation
make docs
cargo doc --open

# Development build and test cycle
make dev

# Additional Makefile commands
make help                    # Show all available commands
make perf-test              # Performance tests with real server
make integration-full       # Full integration test with server
make test-python-install    # Install Python test dependencies
make docker-build           # Build Docker image
make docker-run             # Run ProximaDB in Docker
make docs-update-gaps       # Update critical documentation gaps
```

## Architecture Overview

ProximaDB is a unified intelligence platform combining vector search, graph relationships, and semantic knowledge in a single system. Built with a proto-first architecture for maximum performance.

### Core Architecture Layers

1. **Storage Layer** (`src/storage/`)
   - **Multiple Storage Engines**: SST, VIPER, NOVA, SWIFT, RAPTOR, PRISM, HELIX
   - **Unified Storage Interface**: All engines implement `UnifiedStorageEngine` trait
   - **IntelligentFilesystem**: Unified filesystem abstraction with caching (wraps Local, S3, Azure, GCS)
   - **Write-Ahead Log (WAL)**: Located in `src/storage/persistence/write_ahead_log/`
   - **Metadata Store**: Atomic operations with cloud backend support

2. **Compute Layer** (`src/compute/`)
   - **Unified Quantization**: All engines use `compute::quantization::unified` module
   - **Distance Computation**: Hardware-accelerated SIMD processing (`UnifiedDistanceCompute`)
   - **Quantization Levels**: Binary, INT8, PQ4, PQ8, PQ16, PQ32 with automatic selection
   - **Hardware Acceleration**: Automatic AVX2/AVX512/NEON/SSE detection

3. **API Layer** (`src/api_handlers/`, `src/network/`)
   - **Unified Handlers**: Single implementation for REST and gRPC
   - **Protocol Buffers**: Native VectorRecord flow without serialization overhead
   - **Multi-Server**: Concurrent REST (port 5678) and gRPC (port 5679) servers

4. **Index Layer** (`src/index/`)
   - **AXIS Engine**: Primary indexing system with tiering support
   - **Multiple Index Types**: HNSW, IVF, PQ, FLAT, ANNOY, LSH
   - **Progressive Search**: Multi-tier deduplication and early termination

5. **Services Layer** (`src/services/`)
   - **CollectionService**: Manages vector collections with engine selection
   - **VectorOperationsService**: Direct memtable access for operations
   - **EventLogService**: Persistent event logging for recovery

### Storage Engine Specializations

- **SST Engine**: Row-based, write-optimized with three-stage filtering
  - Best for: Real-time queries, frequent updates
  - Location: `src/storage/engines/impls/sst/`

- **VIPER Engine**: Columnar Parquet format with advanced quantization
  - Best for: Analytics, batch operations, compression
  - Location: `src/storage/engines/impls/viper/`

- **NOVA Engine**: Progressive columnar storage with multi-level quantization
  - Best for: Mixed workloads, progressive search
  - Location: `src/storage/engines/impls/nova/`

- **SWIFT Engine**: High-speed row-based with FastLanes encoding
  - Best for: Low-latency operations
  - Location: `src/storage/engines/impls/swift/`

- **RAPTOR Engine**: Adaptive row-group management with PxK optimization
  - Best for: Dynamic workloads
  - Location: `src/storage/engines/impls/raptor/`

- **PRISM Engine**: Memory-optimized with multi-resolution quantization
  - Best for: Memory-constrained environments
  - Location: `src/storage/engines/impls/prism/`

- **HELIX Engine**: Spiral-pattern storage for time-series data
  - Best for: Temporal data patterns
  - Location: `src/storage/engines/impls/helix/`

### Key Design Patterns

1. **Proto-First Pipeline**: VectorRecord is the native format throughout the system
2. **Zero-Copy Operations**: Direct memory access without intermediate serialization
3. **Unified Quantization**: All engines delegate to `compute::quantization::unified`
4. **Hardware Adaptive**: Automatic CPU/GPU feature detection and optimization
5. **IntelligentFilesystem**: Single abstraction for all storage backends with caching

### Configuration

Primary configuration file: `config/config.toml`

Key configuration sections:
- `[server]`: HTTP/gRPC ports, data directories
- `[storage]`: Engine selection, storage locations, metadata URLs
- `[storage.write_buffer]`: Flush thresholds and memory limits
- `[storage.compaction]`: Background optimization settings
- `[compute.quantization]`: Compression algorithm selection

### Feature Flags
Important Cargo feature flags (use with `--features`):
- `sql_frontend` (default): Modern SQL frontend vs legacy sql_engine
- `cloud-full`: Enable all cloud storage backends (AWS + Azure + GCP)
- `aws`, `azure`, `gcp`: Individual cloud storage backends
- `rocksdb`: RocksDB metadata backend support
- `distributed`, `standalone`: Deployment mode selection
- `gpu`: GPU acceleration support (CUDA, ROCm, MPS, OpenCL)
- `debug-filters`: Enable debug filtering for search operations
- `comprehensive_tests`, `perf_tests`: Extended test suites
- `simd`: SIMD optimization placeholder (ARM NEON support planned)

### Data Directories
- `/data/wal/`: Write-ahead log files
- `/data/metadata/`: Metadata storage with subdirs: current/, archive/, __staging/
- `/data/collections/`: Per-collection engine-specific files
- `/data/viper_data/`: VIPER engine columnar storage

### Multi-Region Strategy
ProximaDB leverages **cloud-native multi-region capabilities** through:
- **IntelligentFilesystem**: Multi-cloud abstraction handles S3 Cross-Region Replication, Azure Geo-Redundancy, GCS Multi-Region buckets
- **Asynchronous Storage Replication**: Cloud providers handle data replication automatically
- **Application Coordination**: ProximaDB coordinates cross-region access through intelligent routing
- **On-Premises**: Incremental rsync for data synchronization between sites
- **No Custom Replication**: Relies on proven cloud provider replication rather than custom implementation

## Important Architecture Concepts

### Quantization System
All storage engines use the unified quantization module at `src/compute/quantization/unified.rs`:
- **UnifiedQuantizationEngine**: Hardware-accelerated quantization with k-means++ clustering
- **Codebook Storage**: Persistent codebook management via `CodebookStore` trait  
- **Multiple Levels**: Binary, INT8, PQ4/8/16/32 with automatic quality selection
- **Hardware Acceleration**: AVX2/AVX512/NEON SIMD optimizations

### Filesystem Architecture
IntelligentFilesystem (`src/storage/persistence/filesystem/intelligent_filesystem.rs`):
- **Unified Interface**: Wraps Local, S3, Azure, GCS filesystems
- **Caching Layer**: Metadata and file content caching with LRU eviction
- **Zero-Copy I/O**: Direct memory mapping where possible
- **Factory Pattern**: FilesystemFactory routes URLs to appropriate implementations

### Storage Engine Integration
All engines implement the `UnifiedStorageEngine` trait with:
- **Common Interface**: Standardized insert, search, flush, compact operations
- **Engine-Specific Optimization**: Each engine optimizes for its use case
- **Metadata Consistency**: Shared metadata format across all engines
- **Cross-Engine Operations**: Engines can delegate operations to each other

## Development Guidelines

### When Fixing Compilation Errors
1. **Check current_error.log**: Always review the latest compilation log with `cargo build 2>&1 | tee current_error.log`
2. **Common Error Patterns**:
   - `struct import 'AxisConfig' is private`: Use public interfaces from index::axis modules
   - `this function takes 1 argument but 0 arguments were supplied`: Check UnifiedDistanceCompute requires DistanceMetric parameter
   - Lifetime errors: Review async/await usage and reference management
3. **Fix by Engine**: Group fixes by storage engine (NOVA, VIPER, SST, SWIFT, RAPTOR, PRISM, HELIX)
4. **Quantization Issues**: All engines should use `compute::quantization::unified`
5. **Filesystem Issues**: All engines should use `IntelligentFilesystem`
6. **Proto Types**: Use internal types, proto conversion only at service boundaries

### Documentation Requirements
1. **AsciiDoc Only**: All documentation must be written in AsciiDoc format (`.adoc` files)
2. **Professional Diagrams**: Use Mermaid diagrams with ProximaDB styling in `[source,mermaid]` blocks
3. **Visual Consistency**: Follow the ProximaDB color scheme and professional styling standards
4. **Structured Content**: Use AsciiDoc features like admonitions, callouts, and proper cross-references
5. **Professional Icons**: Use only approved professional symbols and geometric shapes
6. **Clean Typography**: Avoid casual emojis; use professional symbols and clear text labels
7. **GitHub Integration**: Ensure all Mermaid diagrams render properly in GitHub's AsciiDoc renderer

### Testing Strategy
1. **Rust Unit Tests**: Located in individual modules and `tests/` directory
2. **Integration Tests**: `cargo test --test integration` - test system interactions
3. **Engine-Specific Tests**: Each storage engine has its own test suite
4. **Python SDK Tests**: `clients/python/tests/` - test SDK functionality
5. **Python Integration Tests**: `tests/python/` - comprehensive system tests
6. **Benchmarks**: `benches/` - performance testing with criterion
7. **Current Status**: Use `current_error.log` to track compilation issues

### Test Coverage Requirements
- **Mandatory 50% Coverage**: All new code must achieve minimum 50% test coverage
- **Coverage Verification**: Use `cargo tarpaulin` or similar tools to measure coverage
- **Test-Driven Development**: Write tests first, then implement functionality
- **Coverage Reporting**:
  ```bash
  # Install tarpaulin for coverage reporting
  cargo install cargo-tarpaulin

  # Generate coverage report
  cargo tarpaulin --out Html

  # Coverage with specific threshold
  cargo tarpaulin --fail-under 50
  ```

### Important Files
- `src/lib.rs`: Main library entry point
- `src/bin/server.rs`: Server binary implementation
- `proto/proximadb.proto`: Protocol buffer definitions
- `src/storage/engines/factory.rs`: Storage engine selection logic
- `src/compute/quantization/unified.rs`: Unified quantization engine
- `src/storage/persistence/filesystem/intelligent_filesystem.rs`: Unified filesystem

### Health Checks
```bash
# REST API health check
curl http://localhost:5678/health

# gRPC health check (requires grpc-health-probe)
grpc-health-probe -addr=localhost:5679
```

### Performance Optimization

### Current Benchmark Results (September 2025)
ProximaDB delivers exceptional performance with hardware-accelerated SIMD optimization:

| Dimension | Metric | Throughput (ops/sec) | Latency (μs) |
|-----------|---------|---------------------|--------------|
| **128D** | DotProduct | **20.6M** | **0.049** |
| **128D** | Euclidean | **18.0M** | **0.056** |
| **128D** | Manhattan | **17.7M** | **0.057** |
| **128D** | Cosine | **12.2M** | **0.082** |
| **256D** | DotProduct | **8.4M** | **0.120** |
| **256D** | Euclidean | **7.9M** | **0.126** |
| **512D** | DotProduct | **3.8M** | **0.265** |
| **512D** | Euclidean | **3.7M** | **0.271** |

**Complete Performance Documentation**: See `docs/PERFORMANCE_COMPREHENSIVE.adoc` for detailed benchmarks, competitive analysis, and deployment guidance.

### Hardware Acceleration Features
The system automatically detects and uses:
- SIMD instructions (AVX2/NEON) - **delivering 20M+ ops/sec performance**
- GPU acceleration (CUDA/ROCm/MPS)
- CPU cache sizes for optimal batching
- 13 compression algorithms with context-aware selection

### Available Benchmarks
```bash
# All available benchmarks (use --list to see)
cargo bench

# Specific performance benchmarks
cargo bench --bench simd_distance_bench
cargo bench --bench flush_optimization_bench
cargo bench --bench vector_optimization_bench
cargo bench --bench engine_comparison_bench

# Run with custom timing
cargo bench -- --warm-up-time 1 --measurement-time 5
```

### Python Client SDK
Location: `clients/python/`

Supports automatic protocol selection (REST/gRPC) with:
- Collection management
- Vector insertion/updates
- Similarity search with metadata filtering
- SQL-style queries
- Compression configuration

## Troubleshooting

### Common Issues
1. **Port conflicts**: Check `lsof -i :5678` and kill conflicting processes
2. **Permission issues**: `sudo chown -R $USER:$USER ./data`
3. **ARM64 build issues**: Use `cargo build --no-default-features`
4. **Quantization errors**: Ensure all engines use unified quantization module
5. **Filesystem errors**: Ensure all engines use IntelligentFilesystem
6. **Compilation tracking**: Use `current_error.log` to track ongoing compilation issues

### Debugging Commands
```bash
# Debug logging for specific module
RUST_LOG=proximadb::storage=trace cargo run --bin proximadb-server

# Memory profiling
valgrind --tool=massif cargo run --bin proximadb-server

# Performance profiling (Linux)
perf record --call-graph=dwarf cargo run --release --bin proximadb-server
```