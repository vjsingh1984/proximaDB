# ProximaDB Documentation Index

**Version**: 0.1.5
**Last Updated**: 2025-10-23
**Purpose**: Complete guide to all ProximaDB documentation with quick-find navigation (see `AGENTS.md` for authoritative commands/standards)

---

## Quick Navigation

**New to ProximaDB?** Start here:
1. [Getting Started](#getting-started) - Installation and first steps
2. [Demos](#demos) - Hands-on examples (100% working)
3. [Performance Guide](#performance) - **Single source of truth** for optimization

**Building an application?** Check out:
- [API Reference](#api-reference) - REST and gRPC endpoints
- [Python SDK](#python-sdk) - Client library guide
- [Configuration](#configuration) - Server and storage settings

**Need deep technical details?**
- [Storage Engines](#storage-engines) - 6 specialized engines (feature-gated cloud/GPU paths)
- [Technical Guides](#technical-guides) - Architecture, compression, GPU (feature-gated/experimental noted)
- [Performance Benchmarks](#performance) - Validated measurements (see WAL caveat below)

---

## Table of Contents

- [Getting Started](#getting-started)
- [Demos](#demos)
- [API Reference](#api-reference)
- [Guides](#guides)
- [Storage Engines](#storage-engines)
- [Performance](#performance)
- [Technical Guides](#technical-guides)
- [Operations](#operations)
- [Development](#development)
- [Testing](#testing)
- [Roadmap](#roadmap)
- [Business Documentation](#business-documentation)
- [Session Reports](#session-reports)

---

## Getting Started

### Installation and Setup

**Main Documentation**:
- `docs/01-getting-started/` - Installation guides
- `docs/README.adoc` - Overview and architecture

**Quick Start**:
```bash
# 1. Install ProximaDB
git clone https://github.com/vjsingh1984/proximaDB
cd proximaDB
cargo build --release

# 2. Start server
cargo run --bin proximadb-server

# 3. Install Python SDK
cd clients/python
pip install -e .

# 4. Run first demo
export PYTHONPATH=./clients/python/src
python3 demo/quickstart/basic_demo.py
```

**Key Files**:
- `README.md` - Repository overview
- `CLAUDE.md` - Developer guide for working with the codebase
- `Cargo.toml` - Rust project configuration
- `config/config.toml` - Server configuration

---

## Demos

**Location**: `demo/`
**Status**: ✅ SDK-based demos passing except WAL-path-sensitive flows; `wal_search.py` depends on WAL metadata propagation fix (see `URGENT_FIX_INSTRUCTIONS.md` / `FINAL_FIX_NEEDED.md`)
**Last Validated**: 2025-10-23 (pending WAL pool fix revalidation)

### Demo Documentation

| File | Purpose | Status |
|------|---------|--------|
| `demo/README.md` | Comprehensive demo guide with prerequisites and troubleshooting | ✅ Complete |
| `demo/CONTRIBUTING.md` | Contribution guidelines for new demos | ✅ Complete |
| `demo/check_demo_health.py` | Environment validation tool (7 checks) | ✅ Working |
| `demo/run_all_demos.sh` | Automated test runner for CI/CD | ✅ Working |

### Demo Categories

**Quickstart Demos** (`demo/quickstart/`):
- `basic_demo.py` - Core vector operations (~3s)
- `feature_showcase.py` - Multi-feature overview (~5s)
- `unified_rest_api_demo.py` - Raw REST API (⚠️ requires server fix)

**Feature Showcases** (`demo/showcases/features/`):
- `chunking_demo.py` - 6 text chunking strategies (~8s) ✅
- `metadata_filtering.py` - Server-side filtering via gRPC (~12s) ✅
- `quantization_demo.py` - Vector compression benchmarks (~45s) ✅
- `wal_search.py` - WAL operations and recovery (~6s) ⚠️ depends on WAL metadata propagation fix

**Industry Use Cases** (`demo/showcases/industry/`):
- `ecommerce_demo.py` - E-commerce product search
- `ai_knowledge_base_demo.py` - AI knowledge management

**Advanced Topics** (`demo/showcases/advanced/`):
- `embedding_service.py` - External embedding integration
- `sec_edgar_complete.py` - SEC filing analysis

**Benchmarks** (`demo/benchmarks/performance/`):
- `protocol_comparison.py` - REST vs gRPC performance

### Session Reports

**Demo Infrastructure Work** (October 2025):
- `ALL_DEMOS_FIXED_FINAL_REPORT.md` - Complete fix report (100% SDK success)
- `DEMO_INFRASTRUCTURE_IMPROVEMENTS.md` - Infrastructure enhancements
- `DEMO_FIX_SESSION_RESULTS.md` - Initial fixes session
- `QUANTIZATION_FIX_FINAL_REPORT.md` - Quantization SDK fixes

---

## API Reference

**Location**: `docs/03-reference/`

### REST and gRPC APIs

| Document | Purpose | Key Content |
|----------|---------|-------------|
| `rest-api-specification.adoc` | Complete REST API reference | Endpoints, request/response formats, examples |
| `configuration-reference.adoc` | Configuration options | Server, storage, cache, compute settings |
| `graph_api_reference.adoc` | Graph database API | Node/edge operations, traversal queries |
| `sks_technical_reference.adoc` | Semantic Knowledge Store (SKS) | Hybrid graph-vector queries |

**Quick Reference**:
- REST API: `http://localhost:5678/api/v1/`
- gRPC API: `localhost:5679`
- Health Check: `http://localhost:5678/health`

**Key Endpoints**:
```bash
# Collections
POST   /api/v1/collections              # Create collection
GET    /api/v1/collections              # List collections
GET    /api/v1/collections/{id}         # Get collection
DELETE /api/v1/collections/{id}         # Delete collection

# Vectors
POST   /api/v1/collections/{id}/vectors         # Insert vectors
GET    /api/v1/collections/{id}/vectors/{vid}   # Get vector
POST   /api/v1/collections/{id}/search          # Search vectors
DELETE /api/v1/collections/{id}/vectors/{vid}   # Delete vector

# Graph (SKS)
POST   /api/v1/graph/graphs/{id}/nodes          # Create node
POST   /api/v1/graph/graphs/{id}/edges          # Create edge
POST   /api/v1/graph/graphs/{id}/traverse       # Traverse graph
```

---

## Guides

**Location**: `docs/02-guides/`

### User Guides

- Quick start tutorials
- Feature guides
- Integration patterns
- Best practices

---

## Storage Engines

ProximaDB supports **6 specialized storage engines** optimized for different workloads.

### Engine Documentation

**Engine READMEs** (in `src/storage/engines/impls/{engine}/README.adoc`):

| Engine | Use Case | Document | Key Features |
|--------|----------|----------|--------------|
| **SST** | Real-time OLTP | `sst/README.adoc` | Three-stage filtering, bloom filters, ProximaBlocks |
| **VIPER** | Analytics/OLAP | `viper/README.adoc` | Columnar Parquet, advanced quantization (22-25% compression) |
| **NOVA** | Mixed workloads | `nova/README.adoc` | Progressive search, quantized columns |
| **SWIFT** | High-throughput | `swift/README.adoc` | Superblock caching, ultra-low latency |
| **RAPTOR** | Adaptive workloads | `raptor/README.adoc` | Adaptive PXK, unpredictable access patterns |
| **HELIX** | Spatial locality | `helix/README.adoc` | PCA-optimized, image embeddings |

### Deep Dive Documentation

**Location**: `docs/storage/`

- `sst-engine-deep-dive.adoc` - SST engine internals
- `viper-engine-deep-dive.adoc` - VIPER columnar architecture
- `nova-engine-deep-dive.adoc` - NOVA hybrid design
- `nova-engine-operations.adoc` - NOVA operational guide
- `columnar-storage-architecture.adoc` - General columnar architecture
- `hybrid-columnar-engines.adoc` - Hybrid storage patterns
- `storage_engine_layouts.adoc` - On-disk layouts

### Engine Selection Guide

**Quick Reference** (from performance guide):

```
For Real-Time OLTP:
  → SST engine (block_size=1024KB, compression=lz4)

For Analytics/OLAP:
  → VIPER engine (Parquet columnar, advanced quantization)

For Mixed Workloads:
  → NOVA engine (progressive search)

For Ultra-Low Latency:
  → SWIFT engine (records_per_block=512, compression=none)

For Clustered Data (images, embeddings):
  → HELIX engine (PCA compression, spatial indexing)

For Unpredictable Access:
  → RAPTOR engine (adaptive PXK matrix)
```

---

## Performance

**Primary Document**: `docs/performance/README.adoc` ⭐ **SINGLE SOURCE OF TRUTH**

**Status**: All benchmarks validated October 2024

### What's in the Performance Guide

1. **Quick Reference Tables**:
   - 1K vector scale recommendations
   - 10K vector scale recommendations
   - Engine selection flowcharts
   - Compression decision trees

2. **Validated Configurations** (October 2024):
   - SST: `block_size=1024KB, compression=lz4` (34% faster than 2MB)
   - SWIFT: `records_per_block=512` (11% faster than 2000)
   - HELIX: Best overall performance (90% pruning with spatial indexing)
   - Compression: LZ4 is 7% faster than no compression!

3. **Production Scale Analysis**:
   - See `PRODUCTION_SCALE_ANALYSIS.adoc` for 10K vector deep dive
   - All latencies measured and validated

4. **Archive**:
   - `docs/performance/archive/` - Historical detailed analyses (reference only)

### Key Performance Files

| File | Purpose | Status |
|------|---------|--------|
| `performance/README.adoc` | **Main guide - start here** | ✅ Current |
| `performance/PRODUCTION_SCALE_ANALYSIS.adoc` | 10K vector analysis | ✅ Validated |
| `performance/archive/` | Historical detailed docs | 📦 Archived |

### Benchmark Infrastructure

**Location**: `benches/` and `src/bin/`

- `src/bin/proximadb-bench-consolidated.rs` - Consolidated benchmark binary
- `benches/bench_04_storage_unified.rs` - Unified storage benchmarks
- Individual engine benchmarks in `benches/`

**Running Benchmarks**:
```bash
# Run all benchmarks
cargo bench

# Run specific benchmark
cargo bench --bench bench_04_storage_unified

# Run consolidated benchmark
cargo run --bin proximadb-bench-consolidated --release

# Generate flamegraph
cargo flamegraph --bench bench_04_storage_unified
```

---

## Technical Guides

**Location**: `docs/technical/`

### Core Technical Documentation

| Document | Topic | Key Content |
|----------|-------|-------------|
| `platform_architecture.adoc` | Platform architecture | Layered design, component interaction |
| `proximacodec_technical_guide.adoc` | ProximaCodec compression | 15 encoding schemes, SIMD optimization |
| `compression_guide.adoc` | Compression overview | Algorithms, trade-offs, selection |
| `unified-quantization-guide.adoc` | Vector quantization | Binary, INT8, PQ4/8/16/32 schemes |
| `gpu_acceleration.adoc` | GPU compute | CUDA/Metal integration, kernels |
| `VECTOR_DB_FORMAT_COMPARISON.adoc` | Format comparison | Lance, Nimble analysis vs ProximaDB |
| `SST_COMPETITIVE_COMPARISON.adoc` | SST vs competitors | Feature and performance comparison |

### Specialized Topics

**Graph Database**:
- `graph_collection_service_design.adoc` - Graph service architecture
- `graph_persistence_architecture.adoc` - Graph storage internals

**AI Platform**:
- `ai_intelligence_platform.adoc` - AI-first architecture

**Optimization**:
- `optimization/proxima_simd_optimization_guide.adoc` - SIMD techniques
- `proxima_encoding_differences.adoc` - Encoding trade-offs

**Compression Deep Dives**:
- `compression/proxima_vs_viper_compression_analysis.adoc` - ProximaCodec vs Parquet

---

## Operations

**Location**: `docs/04-operations/`

### Production Operations

| Document | Purpose |
|----------|---------|
| `PRODUCTION_RUNBOOK.adoc` | Operational procedures, monitoring, troubleshooting |
| `benchmark_logging_guide.adoc` | Logging and metrics for benchmarks |

### Configuration Files

**Location**: `config/`

- `config.toml` - Main server configuration
- `test-config.toml` - Test environment config
- `production.toml` - Production settings

**Key Configuration Sections**:
```toml
[server]
bind_address = "0.0.0.0"

[api]
rest_port = 5678
grpc_port = 5679

[storage]
data_directory = "./data"

[storage.sst_config]
block_size_kb = 1024      # Optimized (34% faster)
compression = "lz4"       # Optimized (7% faster!)

[cache]
memory_budget_mb = 1024

[compute]
enable_simd = true
```

---

## Development

**Location**: `docs/08-development/`

### Developer Documentation

- Development workflows
- Contribution guidelines
- Code patterns and standards

### Key Developer Files

| File | Purpose |
|------|---------|
| `CLAUDE.md` | **Main developer guide** - Commands, architecture, patterns |
| `CODE_DUPLICATION_REFACTORING_PLAN.md` | Refactoring analysis (540 lines) |
| `CODE_DUPLICATION_SUMMARY.md` | Quick reference for test utilities |
| `tests/common/collection_builder.rs` | Shared test collection builder |
| `tests/common/vector_generator.rs` | Shared vector generation utilities |

### Build and Test

**Quick Commands** (from CLAUDE.md):
```bash
# Build
cargo build                    # Debug
cargo build --release          # Release
cargo build --profile release-server  # Server-optimized

# Test
cargo test                     # All tests
cargo test --lib               # Library tests only
cargo test test_name           # Specific test
make test-python               # Python SDK tests

# Code Quality
cargo fmt                      # Format
cargo clippy                   # Lint
make check                     # fmt + clippy + test
```

### Test Utilities (NEW - October 2025)

**Shared Test Infrastructure**:
```rust
use tests::common::collection_builder::{TestCollectionBuilder, presets};

// Simple default collection
let (collection, _temp) = TestCollectionBuilder::new().build();

// Custom configuration
let (collection, _temp) = TestCollectionBuilder::new()
    .with_dimension(512)
    .with_engine(StorageEngine::Viper)
    .build();

// Presets
let (collection, _temp) = presets::sst_oltp().build();
```

**Benefits**:
- Reduces test boilerplate from 50-80 lines to 5-10 lines
- Standardized patterns across codebase
- See `tests/example_using_new_utilities.rs` for examples

---

## Testing

**Location**: `docs/testing/`

### Test Documentation

| Document | Purpose |
|----------|---------|
| `IGNORED_TESTS_ANALYSIS.adoc` | Analysis of ignored tests, coverage gaps |

### Test Organization

**Test Directories**:
```
tests/
├── common/                    # ✨ NEW: Shared test utilities
│   ├── collection_builder.rs # Collection builder with presets
│   ├── vector_generator.rs   # Vector generation utilities
│   └── mod.rs                 # Module exports
├── integration/               # Integration tests
├── rust/                      # Rust integration tests
├── python/                    # Python SDK tests
└── example_using_new_utilities.rs  # Example patterns
```

**Python SDK Tests**:
- Location: `clients/python/tests/`
- Requirements: `clients/python/tests/requirements.txt`
- Run: `make test-python` or `python3 -m pytest -v`

---

## Roadmap

**Location**: `docs/09-roadmap/`

### Planning Documentation

**Current Roadmap** (⭐ Start here):
- `implementation/CURRENT_ROADMAP.adoc` - **Roadmap summary and quick reference**
- `MASTER_FEATURE_DASHBOARD.adoc` - Complete feature tracking

### Implementation Plans (Organized by Status)

**Active Plans** (`docs/09-roadmap/implementation/active/`):
- `Q3_2025_IMPLEMENTATION.adoc` - Q3 2025 implementation (in progress)
- `Q4_2025_IMPLEMENTATION.adoc` - Q4 2025 implementation (planned)
- `PRIORITY_ACTION_PLAN.adoc` - Immediate priorities

**Completed Implementations** (`docs/09-roadmap/implementation/completed/`):
- `HOLISTIC_QUANTIZATION_FRAMEWORK.adoc` - Comprehensive quantization (✅ complete)
- `AUTOML_COMPLETE.adoc` - AutoML integration (✅ complete)
- `VIPER_OPTIMIZATION_STRATEGY.adoc` - VIPER engine optimization (✅ complete)
- `QUANTIZED_COLUMNAR_IMPLEMENTATION_PLAN.adoc` - Quantized columnar storage (✅ complete)

**Archived Plans** (`docs/09-roadmap/implementation/archive/`):
- `Q1_2025_IMPLEMENTATION.adoc` - Q1 2025 historical plan
- `Q2_2025_IMPLEMENTATION.adoc` - Q2 2025 historical plan
- `SYSTEM_OPTIMIZATION_GAP_ANALYSIS.adoc` - System optimization analysis
- `SYSTEM_OPTIMIZATION_IMPLEMENTATION.adoc` - System optimization implementation

---

## Business Documentation

**Location**: `docs/10-business/`

### Business Resources

- `competitive_advantages.adoc` - Market positioning
- Use case documentation
- `docs/USE_CASES.adoc` - Application scenarios

---

## Session Reports

**Location**: `docs/sessions/2025/10-october/` (organized by topic)

**Recent Infrastructure Work** (October 2025):

### Demo Infrastructure (`docs/sessions/2025/10-october/demos/`)
- `ALL_DEMOS_FIXED_FINAL_REPORT.md` - Achieved 100% SDK-based demo success
- `DEMO_FIX_FINAL_SUMMARY.md` - Complete demo fix summary
- `DEMO_FIX_SESSION_RESULTS.md` - Initial fixes (chunking, metadata filtering)
- `DEMO_FIX_STATUS.md` - Demo fix status tracking
- `QUANTIZATION_FIX_FINAL_REPORT.md` - Quantization SDK converter fixes

### Demo Infrastructure Development (`docs/sessions/2025/10-october/demo-infrastructure/`)
- `DEMO_INFRASTRUCTURE_COMPLETION_REPORT.md` - Health check, test runner, documentation

### Code Quality (`docs/sessions/2025/10-october/code-quality/`)
- `CODE_DUPLICATION_REFACTORING_PLAN.md` - Comprehensive refactoring analysis
- `CODE_DUPLICATION_SUMMARY.md` - Test utilities quick reference
- `REFACTORING_RESULTS.md` - Refactoring implementation results

### Python SDK (`docs/sessions/2025/10-october/sdk/`)
- `SDK_DIMENSION_FIELD_FIX.md` - Dimension field warning fixes
- `SDK_QUANTIZATION_FIX_COMPLETE.md` - Quantization converter fixes
- `PYTHON_SDK_EXAMPLES_FIXES.md` - Example fixes and improvements

### Graph API (`docs/sessions/2025/10-october/graph-api/`)
- `GRAPH_API_BUG_REPORT.md` - Graph API bug identification
- `GRAPH_API_FIX_SUMMARY.md` - Graph API fix summary
- `GRAPH_API_INTEGRATION_SUMMARY.md` - Graph API integration completion

### Dependencies (`docs/sessions/2025/10-october/dependencies/`)
- `DEPENDENCY_AUDIT_REPORT.md` - Dependency audit and cleanup
- `DEPENDENCY_CLEANUP_SUMMARY.md` - Dependency cleanup results

### General Session Reports (`docs/sessions/2025/10-october/`)
- `SESSION_8_ALL_FIXES_COMPLETE.md` - Session 8 completion summary
- `SESSION_SUMMARY.md` - Overall session summary

### Build and Test Optimization (Permanent Docs)
- `docs/BUILD_OPTIMIZATION.adoc` - Profile optimization analysis
- `docs/OPTIMIZATION_INFRASTRUCTURE_REUSE.adoc` - Build artifact reuse
- `docs/TEST_FAILURES_ROOT_CAUSE_ANALYSIS.adoc` - Test failure investigation

---

## Architecture Documentation

**Location**: `docs/architecture/`

### Architecture Diagrams and Descriptions

- Platform layering
- Component interaction
- Data flow diagrams

---

## Documentation Best Practices

### Finding What You Need

**Use this index to find documentation quickly**:

1. **For getting started**: See [Getting Started](#getting-started)
2. **For running examples**: See [Demos](#demos)
3. **For API usage**: See [API Reference](#api-reference)
4. **For performance tuning**: See [Performance](#performance) (single source of truth)
5. **For engine selection**: See [Storage Engines](#storage-engines)
6. **For technical details**: See [Technical Guides](#technical-guides)
7. **For operations**: See [Operations](#operations)
8. **For development**: See [Development](#development)

### Documentation Structure

ProximaDB follows a **numbered documentation hierarchy**:

```
docs/
├── 01-getting-started/    # Installation, first steps
├── 02-guides/             # User guides and tutorials
├── 03-reference/          # API and configuration reference
├── 04-operations/         # Production operations
├── 06-security/           # Security documentation
├── 08-development/        # Developer guides
├── 09-roadmap/            # Planning and roadmap
│   └── implementation/    # Organized by status (active/completed/archive)
├── 10-business/           # Business documentation
├── performance/           # ⭐ Performance guide (single source)
├── sessions/              # Session reports organized by date and topic
│   └── 2025/10-october/   # October 2025 sessions
├── storage/               # Storage engine deep dives
├── technical/             # Technical architecture
├── testing/               # Test documentation
└── architecture/          # Architecture diagrams
```

### Key Principles

1. **Single Source of Truth**: `docs/performance/README.adoc` is the definitive performance guide
2. **Validated Data**: All performance numbers validated October 2024
3. **Practical Examples**: Every feature has working demos in `demo/`
4. **Developer-Friendly**: CLAUDE.md provides quick command reference
5. **Comprehensive Coverage**: From getting started to advanced internals

---

## Python SDK

**Location**: `clients/python/`

### SDK Documentation

- `clients/python/README.md` - Python SDK overview
- `clients/python/src/proximadb/` - SDK source code
- `clients/python/tests/` - SDK tests
- `demo/` - SDK usage examples

**Installation**:
```bash
cd clients/python
pip install -e .

# Or add to PYTHONPATH
export PYTHONPATH=/path/to/proximaDB/clients/python/src
```

**Quick Start**:
```python
from proximadb import ProximaDBClient, CollectionConfig, DistanceMetric

# Create client
client = ProximaDBClient(url="http://localhost:5678", protocol="rest")

# Create collection
config = CollectionConfig(
    name="my_collection",
    dimension=128,
    distance_metric=DistanceMetric.COSINE
)
collection = client.create_collection("my_collection", config)

# Insert vectors
vectors = [...]  # List of VectorRecord
client.insert_vectors("my_collection", vectors)

# Search
results = client.search(
    collection_id="my_collection",
    vector=query_vector,
    top_k=10
)
```

---

## Related Files

### Repository Root

- `README.md` - Repository overview
- `CLAUDE.md` - **Developer guide** (commands, architecture, patterns)
- `Cargo.toml` - Rust dependencies and project config
- `Makefile` - Build shortcuts and commands

### Documentation Architecture

- `docs/DOCUMENTATION_ARCHITECTURE.adoc` - Documentation organization
- `docs/README.adoc` - Main documentation entry point

---

## Quick Find by Topic

### Performance Optimization
→ `docs/performance/README.adoc` (single source of truth)

### Storage Engine Selection
→ See [Storage Engines](#storage-engines) section above

### API Endpoints
→ `docs/03-reference/rest-api-specification.adoc`

### Configuration Options
→ `docs/03-reference/configuration-reference.adoc`

### Demo Examples
→ `demo/README.md`

### Developer Commands
→ `CLAUDE.md`

### Test Infrastructure
→ `tests/common/` + `CODE_DUPLICATION_SUMMARY.md`

### Build Optimization
→ `docs/BUILD_OPTIMIZATION.adoc`

---

## Getting Help

### Documentation Issues

- **Missing documentation**: Open an issue at https://github.com/vjsingh1984/proximaDB/issues
- **Unclear instructions**: Check CLAUDE.md for developer guidance
- **Performance questions**: See `docs/performance/README.adoc` (single source of truth)
- **Demo problems**: Run `python3 demo/check_demo_health.py --verbose`

### Support Channels

- GitHub Issues: https://github.com/vjsingh1984/proximaDB/issues
- Documentation: This index + linked documents
- Code Examples: `demo/` directory (100% working)

---

## Document Status Legend

- ✅ Complete and current
- ⚠️ Partial or requires update
- 📦 Archived (historical reference)
- ⭐ Primary/recommended document

---

**Last Updated**: 2025-10-23
**Index Version**: 1.0
**Maintained by**: ProximaDB Team

*This index covers all ProximaDB documentation. For the latest updates, see the docs/ directory.*
