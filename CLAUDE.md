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

### Testing Requirements
- **Test Coverage**: Aim for comprehensive testing of new features
- **Unit Tests**: Include tests in module `#[cfg(test)]` blocks
- **Integration Tests**: Located in `tests/` directory
- **Benchmarks**: Performance tests in `benches/` directory
- **Test Commands**: Use `cargo test --lib module::name` for specific tests

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
- `proximadb-bench-consolidated`: Consolidated benchmarking tool (src/bin/proximadb-bench-consolidated.rs)
- `test_bloom_filter`: Bloom filter testing utility (src/bin/test_bloom_filter.rs)
- `test_engine_data_sizes`: Storage engine data size analyzer (src/bin/test_engine_data_sizes.rs)

### Helper Scripts (scripts/)
- `build_and_test.sh`: Full build and test pipeline
- `build_minimal.sh`: Minimal build for quick iteration
- `build-docker.sh`: Docker image build script
- `consolidate_docs.sh`: Documentation consolidation utility
- `run_demo.sh`: Run demo application with sample data
- `deploy_enterprise_release_1.sh`: Enterprise deployment script
- `start_all_emulators.sh`: Start cloud emulators for testing (S3, Azure, GCS)
- `stop_all_emulators.sh`: Stop all cloud emulators

### Available Benchmarks
Located in `benches/` directory - Run with `cargo bench` or specific benchmark:
- Core distance metrics: `cargo bench --bench bench_01_core_distance`
- Storage engines: `cargo bench --bench bench_04_storage_unified`
- System optimization: `cargo bench --bench bench_12_system_optimization`

### Quick Development Workflow
```bash
# Fast compilation check
cargo check --all-targets

# Build with error logging
cargo build 2>&1 | tee current_error.log

# Test specific module
cargo test --lib storage::engines::impls::raptor

# Run single test with debug output
RUST_LOG=debug cargo test test_name -- --exact --nocapture
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

# Run specific test
cargo test test_name -- --exact --nocapture
RUST_LOG=debug cargo test test_name -- --nocapture  # With debug output

# Test specific module
cargo test --lib storage::engines::impls::raptor

# Run benchmarks
cargo bench --bench bench_04_storage_unified
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

ProximaDB is a unified vector database with 6 specialized storage engines that auto-optimize for different workloads.

### Core Components
- **Storage Layer** (`src/storage/`): 6 engines (SST, VIPER, NOVA, SWIFT, RAPTOR, HELIX) implementing `UnifiedStorageEngine`
- **Compute Layer** (`src/compute/`): Unified quantization and hardware-accelerated distance computation
- **API Layer** (`src/api_handlers/`): REST (port 5678) and gRPC (port 5679) servers
- **Index Layer** (`src/index/`): AXIS engine with multiple index types (HNSW, IVF, PQ, etc.)
- **Services Layer** (`src/services/`): CollectionService, VectorOperationsService, EventLogService

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

- **HELIX Engine**: Locality-optimized storage with Hilbert curve clustering
  - Best for: Workloads requiring spatial locality and efficient range queries
  - Location: `src/storage/engines/impls/helix/`

### Key Design Patterns
- Proto-first pipeline with VectorRecord as native format
- Zero-copy operations throughout
- All engines use unified quantization (`compute::quantization::unified`)
- Automatic hardware detection and optimization
- UnifiedCachingFilesystem for all storage backends

### Configuration

Primary configuration file: `config/config.toml`

Key configuration sections in `config/config.toml`:
- `[server]`: HTTP/gRPC ports, data directories
- `[storage]`: Engine selection, storage locations, metadata URLs
- `[storage.write_buffer]`: Flush thresholds and memory limits
- `[storage.compaction]`: Background optimization settings
- `[compute.quantization]`: Compression algorithm selection

### Feature Flags
- `cloud-full`: Enable all cloud storage backends
- `aws`, `azure`, `gcp`: Individual cloud backends
- `rocksdb`: RocksDB metadata backend (optional)
- `gpu`: GPU acceleration (CUDA, ROCm, MPS, OpenCL)

### Data Directories
- `/data/wal/`: Write-ahead log files
- `/data/metadata/`: Metadata storage with subdirs: current/, archive/, __staging/
- `/data/collections/`: Per-collection engine-specific files
- `/data/viper_data/`: VIPER engine columnar storage


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

### Common Compilation Issues
- **Import errors**: Check module paths and public interfaces (e.g., `index::axis` modules)
- **Missing arguments**: `UnifiedDistanceCompute` requires `DistanceMetric` parameter
- **Quantization**: All engines must use `compute::quantization::unified`
- **Filesystem**: All engines must use `UnifiedCachingFilesystem`
- **Proto types**: Use internal types, convert at service boundaries only

### Common Development Patterns
1. **Adding a New Storage Engine**:
   - Implement `UnifiedStorageEngine` trait in `src/storage/engines/impls/`
   - Use `compute::quantization::unified` for quantization
   - Use `UnifiedCachingFilesystem` for all I/O operations
   - Add engine to factory in `src/storage/engines/factory.rs`

2. **Modifying API Endpoints**:
   - Update proto definitions in `proto/proximadb.proto`
   - Run `cargo build` to regenerate proto types
   - Implement handlers in `src/api_handlers/`
   - Keep internal types separate from proto types

3. **Running Tests After Changes**:
   ```bash
   cargo test --lib module_name  # Test specific module first
   cargo test --no-fail-fast     # See all failures at once
   cargo test 2>&1 | tee test_error.log  # Capture output for analysis
   ```


### Testing Strategy
1. **Rust Unit Tests**:
   - Located in individual modules with `#[cfg(test)]`
   - Directory: `tests/unit/` for standalone unit tests

2. **Integration Tests**:
   - Main test: `cargo test --test integration`
   - Files: `tests/integration.rs`, `tests/*_integration_test.rs`
   - Specialized: `tests/graph_integration_test.rs`, `tests/helix_integration_test.rs`, `tests/sks_integration_test.rs`

3. **Engine-Specific Tests**:
   - Directory: `tests/engines/`
   - Each storage engine has dedicated test modules

4. **Python SDK Tests**:
   - Location: `clients/python/test_*.py` (individual test files)
   - No pytest directory structure - run files directly

5. **Test Scripts**:
   - `tests/run_tests.sh`: Comprehensive test runner
   - `tests/run_vector_tests.sh`: Vector operation tests
   - `tests/start_and_test.sh`: Start server and run tests
   - `tests/start_server.sh`: Start server for testing

6. **Specialized Test Categories**:
   - API consistency: `tests/api_consistency_*.rs`
   - Compression: `tests/compression/`
   - Filesystem: `tests/filesystem_*.rs`
   - Graph operations: `tests/graph_*_test.rs`
   - SQL: `tests/sql_*.rs`
   - Security: `tests/security/`
   - Recovery: `tests/recovery/`
   - Metrics: `tests/metrics/`
   - Quantization: `tests/quantization/`

7. **Benchmarks**: `benches/` - performance testing with Criterion
8. **Current Status**: Use `current_error.log` and `test_error.log` to track issues


### Key Files
- `src/storage/engines/factory.rs`: Storage engine selection
- `src/compute/quantization/unified.rs`: Unified quantization
- `src/storage/persistence/filesystem/unified.rs`: Unified filesystem
- `src/network/multi_server.rs`: REST and gRPC servers
- `proto/proximadb.proto`: Protocol definitions
- `config/config.toml`: Main configuration

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

ProximaDB automatically detects and uses hardware acceleration:
- SIMD instructions (AVX2/AVX512/NEON)
- GPU acceleration when available
- Multiple compression algorithms with auto-selection

Benchmarks: `cargo bench` or see `docs/PERFORMANCE_COMPREHENSIVE.adoc` for detailed metrics.

### Python Client SDK

Location: `clients/python/`

Install and test:
```bash
cd clients/python
pip install -e .
python test_v1_client.py  # Run individual test files
```

## Recent Development Context

### Current Development Status (December 2024)
- **Active Branch**: `cleanup_demo` (main branch: `main`)
- **Recent Changes**: Storage engine optimizations, unified cache system, test infrastructure improvements

### Key Recent Changes
- Test infrastructure improvements and systematic error resolution
- Benchmark suite optimization with consistent Criterion settings
- Storage engine stability fixes across all 6 engines
- Documentation compliance with CLAUDE.md specifications
- Unified cache system implementation in `src/storage/cache/`

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