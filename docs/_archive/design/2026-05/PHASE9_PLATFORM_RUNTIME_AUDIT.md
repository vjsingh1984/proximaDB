# Phase 9: Platform Runtime Extraction - Audit Report

**Date**: 2026-05-14
**Status**: Audit Complete - Migration Ready
**Objective**: Extract protocol handlers and server orchestration into platform crates

## Current State Assessment

### Platform Crates Status

**✅ Crates Already Created**:
- `crates/platform/proximadb-api/` - Protocol adapter crate
- `crates/platform/proximadb-runtime/` - Runtime composition crate

**Current Content**:
- `proximadb-api`: Placeholder modules (~265 lines in grpc, ~200 lines in rest)
- `proximadb-runtime`: Composition, hardware, proto_defaults, resources modules

**What's Missing**:
- Actual protocol handler implementations (still in `src/network/`)
- Server bootstrap and orchestration logic (still in `src/`)
- Service composition and wiring (still in `src/`)

---

## Protocol Handler Audit

### REST Handlers (`src/network/rest/`)

**Location**: `src/network/rest/`
**Size**: ~22,422 lines
**Structure**:
```
src/network/rest/
├── mod.rs
├── server.rs
├── health.rs
├── v1/
│   ├── mod.rs
│   ├── handlers.rs
│   ├── aql.rs
│   ├── analytics.rs
│   ├── catalog.rs
│   ├── collection.rs
│   ├── document.rs
│   ├── entity.rs
│   ├── graph.rs
│   ├── hybrid_search.rs
│   ├── logs.rs
│   ├── metrics.rs
│   ├── progressive_search.rs
│   └── vector.rs
└── v2/
    ├── mod.rs
    ├── agentic.rs
    └── (agentic endpoints)
```

**Dependencies**:
- Axum web framework
- Protocol buffer types (proximadb-proto)
- Query runtime contracts
- Core services (via UnifiedHandlers)
- Network middleware

**Migration Complexity**: HIGH
- Many service dependencies
- Complex request/response mapping
- Authentication and middleware integration
- Health checks and metrics

### gRPC Handlers (`src/network/grpc/`)

**Location**: `src/network/grpc/`
**Size**: ~6,682 lines
**Structure**:
```
src/network/grpc/
├── mod.rs
├── collection_service.rs
├── document_service.rs
├── entity_service.rs
├── graph_service.rs
├── hybrid_search_service.rs
├── observability_service.rs
├── security_service.rs
├── sql_service.rs
├── streaming_service.rs
└── vector_service.rs
```

**Dependencies**:
- Tonic gRPC framework
- Protocol buffer types (proximadb-proto)
- Query runtime contracts
- Core services
- Streaming infrastructure

**Migration Complexity**: MEDIUM
- Cleaner separation than REST
- Well-defined service interfaces
- Protocol buffer types already extracted

### PostgreSQL Wire Protocol (`src/network/postgres/`)

**Location**: `src/network/postgres/`
**Size**: ~4,332 lines
**Structure**:
```
src/network/postgres/
├── mod.rs
├── server.rs
├── wire protocol handlers
├── pgvector compatibility layer
└── SQL frontend integration
```

**Dependencies**:
- PostgreSQL wire protocol implementation
- SQL parser and frontend
- Query runtime contracts
- Type system integration

**Migration Complexity**: HIGH
- Complex protocol state machine
- Tight coupling to SQL frontend
- Type system integration

### Arrow Flight (`src/network/arrow/`, `src/network/arrow_ipc/`)

**Location**: `src/network/arrow/`, `src/network/arrow_ipc/`
**Size**: Not measured (appears smaller)
**Structure**:
```
src/network/arrow/
├── Flight protocol handlers
└── Arrow columnar data exchange
```

**Dependencies**:
- Arrow Flight protocol
- Arrow data format
- Query runtime contracts

**Migration Complexity**: MEDIUM
- Well-defined protocol
- Clean data flow

### Network Common (`src/network/`)

**Location**: `src/network/` (top-level files)
**Size**: ~27,770 lines
**Structure**:
```
src/network/
├── mod.rs
├── multi_server.rs (3,500+ lines)
├── server_builder.rs
├── router.rs
├── metrics_service.rs
├── hybrid_search.rs
└── middleware/
    ├── auth.rs
    ├── cors.rs
    ├── backpressure.rs
    ├── rate_limit.rs
    ├── tls.rs
    ├── request_id.rs
    ├── timeout.rs
    ├── tenant.rs
    └── mod.rs
```

**Dependencies**:
- All protocol handlers
- Server lifecycle management
- Middleware stack
- Configuration and TLS

**Migration Complexity**: HIGH
- Central orchestration point
- Complex service composition
- Middleware pipeline

---

## Server Orchestration Audit

### Server Bootstrap (`src/bin/server.rs`, `src/network/server_builder.rs`)

**Location**: `src/bin/server.rs`, `src/network/server_builder.rs`
**Size**: ~1,000+ lines
**Responsibilities**:
- Parse command-line arguments
- Load configuration
- Initialize storage engines
- Start protocol servers
- Graceful shutdown

**Migration Complexity**: HIGH
- Entry point coordination
- Complex initialization sequence
- Resource management

### Service Composition (`SharedServices`, various service constructors)

**Location**: Scattered across `src/services/`, `src/network/`, `src/`
**Size**: Unknown (requires grep audit)
**Responsibilities**:
- Dependency injection
- Service lifecycle
- Capability registration
- Health management

**Migration Complexity**: VERY HIGH
- Distributed across codebase
- Complex dependency graph
- Tight coupling to root crate

### Security Integration (`src/security/`, `src/network/auth/`, `src/network/middleware/`)

**Location**: Multiple directories
**Size**: Unknown (requires grep audit)
**Responsibilities**:
- Authentication (JWT, API keys)
- Authorization (RBAC/ABAC)
- Tenant isolation
- TLS/mTLS
- Audit logging

**Migration Complexity**: HIGH
- Cross-cutting concerns
- Policy enforcement
- Certificate management

### Cluster Orchestration (`src/cluster/`, distributed coordination)

**Location**: `src/cluster/` (if exists)
**Size**: Unknown (requires grep audit)
**Responsibilities**:
- Node membership
- Consensus/coordination
- Shard placement
- Replication control
- Failover and recovery

**Migration Complexity**: VERY HIGH
- Distributed systems complexity
- Consensus protocols
- State management

---

## Dependency Analysis

### Current Dependency Flow (Problematic)

```
src/bin/server.rs
  ↓
src/network/multi_server.rs
  ↓
src/network/rest/, grpc/, postgres/, arrow/
  ↓
src/api_handlers/ (business logic)
  ↓
src/services/ (service implementations)
  ↓
src/query/, src/storage/, src/graph/ (runtime)
```

**Issues**:
- Too much coupling through root crate
- Business logic mixed with protocol handling
- Service composition scattered
- Hard to test in isolation

### Target Dependency Flow (Desired)

```
apps/proximadb-server/bin/main.rs
  ↓
proximadb-runtime (bootstrap, composition)
  ↓
proximadb-api (protocol handlers)
  ↓
proximadb-query (query runtime)
  ↓
proximadb-graph, proximadb-vector (modalities)
  ↓
proximadb-proto, proximadb-kernel (foundation)
```

**Benefits**:
- Clean layering
- Protocol handlers depend on contracts, not concrete services
- Server composition isolated
- Easy to test and extend

---

## Migration Risk Assessment

### High Risk Items

1. **Service Composition** (`SharedServices`)
   - Used across entire codebase
   - Complex dependency graph
   - Hard to extract without breaking changes

2. **REST Handlers** (22,422 lines)
   - Large codebase
   - Many service dependencies
   - Complex middleware integration

3. **Server Bootstrap** (entry point)
   - Critical path
   - Resource initialization
   - Error handling

### Medium Risk Items

1. **gRPC Handlers** (6,682 lines)
   - Cleaner interfaces
   - Protocol buffers already extracted
   - Manageable dependencies

2. **PostgreSQL Wire** (4,332 lines)
   - Self-contained protocol
   - SQL frontend coupling
   - Type system integration

3. **Security/Middleware**
   - Cross-cutting concerns
   - Policy enforcement
   - Authentication/authorization

### Lower Risk Items

1. **Arrow Flight**
   - Well-defined protocol
   - Smaller codebase
   - Clean data flow

2. **Network Common Infrastructure**
   - Mostly structural
   - Reusable patterns

---

## Recommended Migration Strategy

### Option 1: Incremental Protocol-by-Protocol (RECOMMENDED)

**Approach**: Move one protocol handler at a time with compatibility shims

**Sequence**:
1. gRPC handlers (cleanest, medium complexity)
2. Arrow Flight (well-defined, smaller)
3. PostgreSQL wire (self-contained)
4. REST handlers (largest, most complex)
5. Network common infrastructure
6. Server bootstrap and orchestration

**Pros**:
- Manageable risk
- Can test each protocol independently
- Easy to rollback if needed
- Natural staging points

**Cons**:
- Longer timeline
- More compatibility shims needed
- Potential duplicate code during transition

**Estimated Time**: 4-6 weeks

### Option 2: Big-Bang Platform Runtime Extraction

**Approach**: Create complete `proximadb-api` and `proximadb-runtime` in one go

**Sequence**:
1. Extract all protocol handlers to `proximadb-api`
2. Extract all orchestration to `proximadb-runtime`
3. Create `apps/proximadb-server` binary
4. Update all imports across codebase
5. Remove root crate implementations

**Pros**:
- Clean break from root crate
- No incremental compatibility issues
- Final structure achieved quickly

**Cons**:
- Very high risk
- Hard to test incrementally
- Difficult to rollback
- May block development for extended period

**Estimated Time**: 2-3 weeks (but with higher risk)

### Option 3: Layer-First Extraction (ALTERNATIVE)

**Approach**: Extract by layer instead of by protocol

**Sequence**:
1. Move middleware to `proximadb-api/middleware`
2. Move server builder to `proximadb-runtime/bootstrap`
3. Move service composition to `proximadb-runtime/composition`
4. Then extract protocol handlers on top

**Pros**:
- Builds foundation first
- Protocol handlers benefit from extracted infrastructure
- Easier to test

**Cons**:
- Less direct value
- Still need protocol extraction
- More intermediate states

**Estimated Time**: 5-7 weeks

---

## Recommendation

**Proceed with Option 1: Incremental Protocol-by-Protocol Migration**

This approach aligns with:
- Workspace refactor philosophy (gradual, compatible migration)
- Low-risk, high-value extraction
- Ability to test and validate at each step
- Natural staging points for rollback

**First Target**: gRPC handlers (6,682 lines)
- Clean interfaces
- Protocol buffers already extracted
- Medium complexity
- Good proving ground

**Success Criteria**:
- gRPC handlers compile and run from `proximadb-api`
- All gRPC tests pass
- No performance regression
- Root crate becomes compatibility shim
- Workspace boundaries check passes
