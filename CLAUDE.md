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
- **Theme Compatibility**: Use colors that work in both light and dark browser themes
- **AsciiDoc Integration**: Mermaid diagrams must be embedded using `[source,mermaid]` blocks:

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
- **Text Colors**: `#000000` (on colored fills for visibility), `#333333` (on light backgrounds)
- **Border Colors**: Use medium tones that contrast with both light and dark backgrounds
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

**Flow Diagram Template**:
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

**Sequence Diagram Template**:
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

# Check for compilation errors
cargo build 2>&1 | tee current_error.log
cargo check --all-targets  # Faster compilation check without generating binaries
```

### Available Binaries
- `proximadb-server`: Main database server (src/bin/server.rs)
- `proximadb-bench`: Benchmarking tool (src/bin/proximadb-bench-consolidated.rs)

### Available Benchmark Suites (benches/)
- `bench_01_core_distance`: Core distance metrics performance
- `bench_02_hardware_simd`: SIMD hardware acceleration tests
- `bench_03_memory_vector`: Memory and vector operations
- `bench_04_storage_unified`: Unified storage engine benchmarks
- `bench_08_quantization_sst`: SST quantization performance
- `bench_09_columnar_viper`: VIPER columnar operations
- `bench_10_query_progressive`: Progressive query optimization
- `bench_12_system_optimization`: System-wide optimizations
- `bench_13_complete_suite`: Full benchmark suite
- `bench_14_graph_operations`: Graph operations performance

### Quick Development Workflow
```bash
# Fast iteration cycle for development
cargo check --all-targets  # Quick compilation check without binaries
cargo build 2>&1 | tee current_error.log  # Build with error logging
cargo test --lib compute::quantization --no-capture  # Test specific modules
cargo clippy -- -D warnings  # Lint check
```

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

# Performance tests with server
make perf-test

# Full integration tests with real server
make integration-full

# Run specific test category
cargo test --test integration storage::
cargo test --lib compute::quantization  # Test specific module

# Single test with debug output
RUST_LOG=debug cargo test test_name -- --nocapture
cargo test test_name -- --exact --nocapture  # Exact test name matching

# Run specific benchmark
cargo bench --bench bench_01_core_distance
cargo bench --bench bench_04_storage_unified

# Test with specific features
cargo test --features "aws azure gcp"

# Run ignored/slow tests
cargo test -- --ignored

# Test coverage (requires cargo-tarpaulin)
cargo tarpaulin --out Html --fail-under 50

# Run unit tests in specific directory
env PYTHONPATH=src python -m pytest tests/unit/ -v --tb=short
env PYTHONPATH=src python -m pytest tests/unit/ -v --tb=short -m "not (server_required or integration)"
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
cargo fmt -- --check  # Check formatting without modifying files

# Lint with clippy
make clippy
cargo clippy -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings  # Complete linting

# Full quality check (format + lint + test)
make check

# Run benchmarks
make benchmark
cargo bench

# Specific benchmarks
make benchmark-vector
make benchmark-metadata
cargo bench --bench simd_distance_bench
cargo bench --bench flush_optimization_bench
cargo bench --bench engine_comparison_bench
cargo bench -- --warm-up-time 1 --measurement-time 5  # Custom timing

# Run benchmark binary
cargo run --bin proximadb-bench

# Generate documentation
make docs
cargo doc --open
cargo doc --no-deps --document-private-items  # Include private items

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
   - **UnifiedCachingFilesystem**: Consolidated filesystem with integrated caching for Local, S3, Azure, GCS
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
5. **UnifiedCachingFilesystem**: Consolidated abstraction for all storage backends with integrated caching

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
- `sql_frontend` (default): Modern SQL frontend.
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
- **UnifiedCachingFilesystem**: Multi-cloud abstraction handles S3 Cross-Region Replication, Azure Geo-Redundancy, GCS Multi-Region buckets
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
UnifiedCachingFilesystem (`src/storage/persistence/filesystem/unified.rs`):
- **Consolidated Interface**: Single entry point for all filesystem operations
- **Integrated Caching**: Unified metadata cache, disk cache, and prefetch engine
- **Zero-Copy I/O**: Integrated ZeroCopyIOSystem for optimized I/O operations
- **Engine-Aware**: Engine-specific metadata serialization for optimal caching
- **Factory Pattern**: FilesystemFactory.get_unified_caching_filesystem() for instantiation

### Storage Engine Integration
All engines implement the `UnifiedStorageEngine` trait with:
- **Common Interface**: Standardized insert, search, flush, compact operations
- **Engine-Specific Optimization**: Each engine optimizes for its use case
- **Metadata Consistency**: Shared metadata format across all engines
- **Cross-Engine Operations**: Engines can delegate operations to each other

### Cache Architecture (Recent Update)
The caching system has been recently unified (`src/storage/cache/`):
- **CacheOrchestrator**: Central coordination of all cache subsystems
- **VectorCache**: Specialized cache for vector operations (`src/storage/cache/specialized/vector_cache.rs`)
- **EvictionPolicy**: LRU, LFU, and Adaptive strategies
- **Integration Points**: All storage engines now use the unified cache through `CacheOrchestrator`
- **Recent Fix**: Cache module conflicts were resolved by consolidating duplicate implementations

## Development Guidelines

### When Fixing Compilation Errors
1. **Check current_error.log**: Always review the latest compilation log with `cargo build 2>&1 | tee current_error.log`
2. **Recent Development Focus**: The codebase has undergone systematic compilation error fixes and engine completion as of September 2025
3. **Common Error Patterns**:
   - `struct import 'AxisConfig' is private`: Use public interfaces from index::axis modules
   - `this function takes 1 argument but 0 arguments were supplied`: Check UnifiedDistanceCompute requires DistanceMetric parameter
   - Lifetime errors: Review async/await usage and reference management
   - `cannot find type X in this scope`: Check module imports and feature flags
   - `trait bound not satisfied`: Verify trait implementations and generic constraints
4. **Fix by Engine**: Group fixes by storage engine (NOVA, VIPER, SST, SWIFT, RAPTOR, PRISM, HELIX)
5. **Quantization Issues**: All engines should use `compute::quantization::unified`
6. **Filesystem Issues**: All engines should use `UnifiedCachingFilesystem`
7. **Proto Types**: Use internal types, proto conversion only at service boundaries
8. **Quick Error Resolution**:
   ```bash
   cargo check --all-targets 2>&1 | head -20  # Quick check first errors
   cargo build --message-format=short  # Concise error output
   cargo fix --allow-dirty  # Auto-fix some common issues
   ```

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
- `src/bin/proximadb-bench-consolidated.rs`: Consolidated benchmarking binary
- `proto/proximadb.proto`: Protocol buffer definitions
- `src/storage/engines/factory.rs`: Storage engine selection logic
- `src/compute/quantization/unified.rs`: Unified quantization engine
- `src/storage/persistence/filesystem/unified.rs`: Unified caching filesystem
- `src/storage/persistence/filesystem/mod.rs`: Filesystem factory and exports
- `build.rs`: Protocol buffer compilation and build configuration
- `Cargo.toml`: Dependencies and feature flags configuration
- `config/config.toml`: Main server configuration file
- `src/storage/cache/`: Unified cache system with VectorCache specialization
- `src/network/multi_server.rs`: Concurrent REST and gRPC server implementation

### Health Checks and API Testing
```bash
# REST API health check
curl http://localhost:5678/health

# gRPC health check (requires grpc-health-probe)
grpc-health-probe -addr=localhost:5679

# Create collection via REST
curl -X POST http://localhost:5678/v1/collections \
  -H "Content-Type: application/json" \
  -d '{"name": "test_collection", "dimension": 1536}'

# Insert vectors via REST
curl -X POST http://localhost:5678/v1/collections/test_collection/vectors \
  -H "Content-Type: application/json" \
  -d '{"vectors": [{"id": "1", "values": [0.1, 0.2, ...], "metadata": {"key": "value"}}]}'

# Search vectors
curl -X POST http://localhost:5678/v1/collections/test_collection/search \
  -H "Content-Type: application/json" \
  -d '{"query_vector": [0.1, 0.2, ...], "top_k": 10}'

# View dashboard
open http://localhost:5678/dashboard
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

Testing Python SDK:
```bash
# Install Python SDK in development mode
cd clients/python
pip install -e .

# Run Python SDK tests
pytest tests/ -v

# Run specific Python test files
python test_v1_client.py
python test_grpc_simple.py
```

## Recent Development Context

### Current Development Status (September 2025)
- **Active Branch**: Working on `cleanup_demo` branch (main branch is `main`)
- **Recent Focus**: Cache system unification and engine completion
- **Modified Files**: Cache modules, storage engines, multi-server implementation
- **Recent Commits**: Cache integration fixes, Mermaid syntax corrections, documentation improvements

### Key Recent Changes
- Unified cache system implementation in `src/storage/cache/`
- VectorCache specialization added for optimized vector operations
- All storage engines now integrated with CacheOrchestrator
- Documentation migrated to AsciiDoc format with Mermaid diagrams

## Troubleshooting

### Common Issues
1. **Port conflicts**:
   ```bash
   lsof -i :5678  # Check what's using the port
   kill -9 $(lsof -t -i :5678)  # Kill process using port
   ```
2. **Permission issues**:
   ```bash
   sudo chown -R $USER:$USER ./data
   chmod -R 755 ./data  # Fix permissions
   ```
3. **ARM64 build issues**:
   ```bash
   cargo build --no-default-features
   cargo build --target aarch64-apple-darwin  # Explicit ARM64 target on macOS
   ```
4. **Quantization errors**: Ensure all engines use unified quantization module
5. **Filesystem errors**: Ensure all engines use UnifiedCachingFilesystem
6. **Compilation tracking**: Use `current_error.log` to track ongoing compilation issues
7. **Dependency issues**:
   ```bash
   cargo update  # Update dependencies
   cargo clean && cargo build  # Clean rebuild
   rm -rf ~/.cargo/registry/cache  # Clear cargo cache if corrupted
   ```
8. **Test failures**:
   ```bash
   cargo test -- --test-threads=1  # Run tests sequentially to avoid race conditions
   rm -rf /tmp/proximadb  # Clean test data
   ```

### Debugging Commands
```bash
# Debug logging for specific module
RUST_LOG=proximadb::storage=trace cargo run --bin proximadb-server
RUST_LOG=proximadb::compute::quantization=debug cargo run --bin proximadb-server
RUST_LOG=debug,hyper=info,tower_http=info cargo run --bin proximadb-server  # Debug with less HTTP noise

# Backtrace on panic
RUST_BACKTRACE=1 cargo run --bin proximadb-server
RUST_BACKTRACE=full cargo run --bin proximadb-server  # Full backtrace

# Memory profiling
valgrind --tool=massif cargo run --bin proximadb-server
valgrind --leak-check=full cargo run --bin proximadb-server  # Memory leak detection

# Performance profiling (Linux)
perf record --call-graph=dwarf cargo run --release --bin proximadb-server
perf report  # View profiling results

# macOS profiling with Instruments
cargo build --release
instruments -t "Time Profiler" target/release/proximadb-server

# Thread debugging
RUST_LOG=tokio=trace cargo run --bin proximadb-server  # Async runtime debugging
```