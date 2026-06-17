# ADR 001: Workspace Layering Architecture

**Status**: Accepted
**Date**: 2026-05-13
**Decision**: Establish strict layering rules for workspace architecture
**Related**: Phase 5 - Layering Enforcement, Workspace Refactor Plan, Data AI Platform Anchor

---

## Context

ProximaDB has grown organically with unclear dependency boundaries between modules. This has led to:
- Circular dependencies between layers
- Upward dependencies from lower to higher layers
- Difficulty understanding module responsibilities
- Challenges in maintaining and extending the codebase

The workspace refactor has made significant progress in cleaning up "unified" module proliferation and extracting foundation/query/modality seams, but layering needs automated enforcement so new work follows the architecture anchor and workspace refactor plan.

---

## Decision

We establish a strict **layered architecture** with clear dependency rules.

### Layer Hierarchy (Bottom to Top)

1. **Foundation Layer** (`crates/foundation/`, `src/core/foundation/`)
   - Basic types, errors, utilities
   - No dependencies on other ProximaDB layers
   - May depend on external crates (serde, thiserror, etc.)

2. **Contracts Layer** (`crates/contracts/`, `src/proto/`, `src/traits/`)
   - Trait definitions, protocol buffers
   - Type definitions for cross-layer communication
   - No implementation details, only interfaces
   - May depend on Foundation layer only

3. **Modality Runtime Layer** (`crates/modalities/`, `src/compute/`, `src/storage/`)
   - Vector, graph, document, compression runtimes
   - Storage format implementations (Parquet, SST, ProximaBlocks)
   - May depend on Contracts and Foundation layers

4. **Cross-Model Query Runtime** (`src/query/`, `src/graph/query/`)
   - Query planning, optimization, execution
   - Multi-model query orchestration
   - May depend on Modality Runtime, Contracts, Foundation layers

5. **Platform Runtime Layer** (`src/network/`, `src/server/`, `src/api_handlers/`)
   - REST/gRPC servers, protocol handlers
   - Request routing, API endpoints
   - May depend on all lower layers

6. **Apps/Bindings Layer** (`clients/`, `src/embedded/`)
   - CLI tools, SDKs, embedded bindings
   - May depend on any layer as needed

### Golden Rule

**Downward dependencies only**: Higher layers may depend on lower layers, but lower layers must NEVER depend on higher layers.

### Forbidden Patterns

1. **Upward Dependencies**: Lower layers importing from higher layers
   ```rust
   // ❌ FORBIDDEN
   use crate::network::rest::server;  // Foundation importing from Platform
   use crate::query::executor;  // Foundation importing from Query Runtime
   ```

2. **Circular Dependencies**: Two modules depending on each other
   ```rust
   // ❌ FORBIDDEN
   // mod_a depends on mod_b
   // mod_b depends on mod_a
   ```

3. **Implementation in Contracts**: Putting implementations in contract modules
   ```rust
   // ❌ FORBIDDEN
   // src/traits/storage.rs contains concrete storage engine implementations
   ```

4. **Type Definitions in Wrong Layer**: Domain types defined in infrastructure modules
   ```rust
   // ❌ FORBIDDEN
   // Network layer defining domain types that should be in Foundation
   ```

### Correct Patterns

1. **Downward Dependencies**: Higher layers importing from lower layers
   ```rust
   // ✅ CORRECT
   // Platform Runtime importing from Contracts
   use crate::proto::proximadb_v1;
   use crate::storage::traits::UnifiedStorageEngine;
   ```

2. **Trait-Based Boundaries**: Using traits to define layer boundaries
   ```rust
   // ✅ CORRECT
   // Define trait in Contracts layer
   // Implement in Modality Runtime layer
   // Use in Platform Runtime layer
   ```

3. **Foundation Types**: Generic, reusable types in Foundation layer
   ```rust
   // ✅ CORRECT
   // DistanceMetric, QuantizationLevel, CompressionAlgorithm in Foundation
   // Used by all higher layers
   ```

---

## Rationale

### Benefits

1. **Clear Separation of Concerns**: Each layer has a well-defined responsibility
2. **Reduced Coupling**: Lower layers are independent and reusable
3. **Easier Testing**: Layers can be tested in isolation
4. **Better Compilation**: Fewer circular dependencies, faster builds
5. **Scalability**: Easier to add new features without breaking existing code

### Why This Architecture?

1. **Foundation Independence**: Foundation types are reusable across projects
2. **Contract Flexibility**: Traits/proto definitions can be swapped without changing implementations
3. **Modularity**: Each storage modality can evolve independently
4. **Platform Agnostic**: Core logic doesn't depend on network/server implementations

---

## Consequences

### Positive

1. **Reduced Compilation Time**: Fewer circular dependencies, better incremental compilation
2. **Easier Onboarding**: New developers can understand the codebase faster
3. **Better Testability**: Each layer can be tested in isolation
4. **Flexibility**: Easier to swap implementations (e.g., different storage engines)
5. **Code Reusability**: Foundation and contract layers can be reused in other projects

### Negative

1. **More Indirection**: Some code may need additional abstraction layers
2. **Boilerplate**: Need to define traits/interfaces between layers
3. **Learning Curve**: Team needs to understand layering rules
4. **Refactoring Effort**: Existing code may need to be restructured

### Migration Path

1. **Phase 1-3**: Foundation types, core types, storage engines (✅ Complete)
2. **Phase 4**: "Unified" module cleanup (✅ Complete)
3. **Phase 5**: Layering enforcement (⏳ In Progress)
   - Add CI checks to prevent violations
   - Add linter rules
   - Team training
   - Gradual enforcement of rules

---

## Implementation

### Phase 5.1: Documentation (This Phase)

- ✅ Updated CLAUDE.md with layering rules
- ✅ Created this ADR
- ⏳ Update WORKSPACE_REFACTOR_PLAN with layering information

### Phase 5.2: CI Checks

- ✅ Use `scripts/check_workspace_boundaries.py --strict` as the canonical policy checker
- ✅ Add `scripts/check-layering.sh` as a thin wrapper for local hooks and CI
- ✅ Add `.github/workflows/layering-check.yml` workflow
- Fail PRs on layering violations

### Phase 5.3: Linter Rules (Next)

- Create Clippy lints for layering violations
- Configure lints in `clippy.toml`
- Enable in CI

### Phase 5.4: Team Training (Next)

- Create training materials
- Conduct training session
- Update onboarding docs
- Add layering quiz

---

## Alternatives Considered

### Alternative 1: No Layering Rules

**Pros**: No restrictions on code organization
**Cons**: Continued accumulation of technical debt, unclear dependencies

**Rejected**: Phase 4 audit revealed severe layering violations that must be addressed

### Alternative 2: Microservices Architecture

**Pros**: Strict boundaries between services
**Cons**: Increased operational complexity, network overhead, distributed transaction challenges

**Rejected**: ProximaDB is a monolithic database (by design), microservices would add unnecessary complexity

### Alternative 3: Monolithic (No Layers)

**Pros**: Simple, no restrictions
**Cons**: Tight coupling, difficult to maintain, hard to test

**Rejected**: Phase 4 showed that unorganized growth leads to 70+ "unified" modules and circular dependencies

---

## References

- [Workspace Refactor Plan](../../roadmap/techdebt/WORKSPACE_REFACTOR_PLAN_2026_05_07.adoc)
- [Multi-Model Overhaul Spec](../../roadmap/MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc)
- [Data AI Platform Anchor](DATA_AI_PLATFORM_ARCHITECTURE_ANCHOR_2026_05_12.adoc)
- [Phase 4 Completion Report](PHASE_4_COMPREHENSIVE_COMPLETION_REPORT.md)

---

## Revision History

- **2026-05-13**: Initial version - ADR-001 accepted as part of Phase 5.1
