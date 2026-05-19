# ProximaDB Vector Framework SOLID Redesign

## Executive Summary

Based on comprehensive benchmark analysis of all 6 storage engines (SST, HELIX, VIPER, SWIFT, NOVA, RAPTOR), this document proposes a SOLID-based redesign of the vector framework to:

1. **Eliminate ~2,900 lines of duplicated code** (62% reduction)
2. **Unify progressive search across all engines** (4 implementations → 1)
3. **Create capability-driven RL planner** (6 engine-specific files → 1 unified)
4. **Enable natural performance improvements across all verticals**

---

## Benchmark Analysis Insights

### Current Performance (from `/tmp/all_engines_benchmark.log`)

| Engine | 2K Latency | 10K Latency | Best Action | Issue |
|--------|------------|-------------|-------------|-------|
| SWIFT | 4.36ms | 18.46ms | Binary→INT8→FP32 | No shared quant |
| RAPTOR | 5.22ms | 20.96ms | Sqrt/Centroid prune | Isolated pruning |
| HELIX | 7.81ms | 26.81ms | HNSW + Progressive | Disconnected PCA |
| VIPER | 8.26ms | 34.25ms | Ratio(0.5) prune | No AXIS integration |
| SST | 8.61ms | 19.76ms | DirectScan→HNSW | Good integration |
| NOVA | 8.60ms | 46.30ms | IVF + Adaptive | Poor scaling |

### RL Planner Action Diversity (from logs)

```
Actions Observed:
- Index: DirectScan, HNSW(m=16, ef=50), HNSW(m=16, ef=100), IVF(nprobe=16)
- Mode: Exact, Approximate(exp=1), Approximate(exp=2), Adaptive(thresh=5000)
- Quant: FP32, Binary→FP32, Binary→INT8→FP32, Binary→INT8→PQ8→FP32
- Prune: None, Sqrt, Ratio(0.5), Centroid(thresh=1.5)
```

**Key Insight**: The RL planner explores many strategies, but engines can't leverage cross-engine optimizations because each implements its own isolated version.

---

## SOLID Principle Violations Identified

### 1. Single Responsibility Principle (SRP) Violations

| Component | Current Responsibilities | Should Be |
|-----------|-------------------------|-----------|
| `UnifiedStorageEngine` | 27 methods (search, flush, compact, metrics, validation, staging) | 5 focused traits |
| `StorageQueryContext` | Query params + collection metadata + quant config + performance hints | 3 separate contexts |
| `search_vectors_unified()` | Strategy selection + execution + result processing | Delegated to strategies |

### 2. Open/Closed Principle (OCP) Violations

**Location**: `src/storage/traits.rs:639-737`
```rust
// PROBLEM: Adding new engine requires modifying base trait
match self.strategy() {
    StorageEngineStrategy::Sst => ScanCapabilities { ... },
    StorageEngineStrategy::Viper => ScanCapabilities { ... },
    // ... 6 engines hardcoded
}
```

### 3. Liskov Substitution Principle (LSP) Violations

Engines aren't truly substitutable - each has different search semantics:
- SST: Requires bloom filters
- VIPER: Mandates row groups
- HELIX: Needs PCA prerequisite
- RAPTOR: Has consolidation overhead

### 4. Interface Segregation Principle (ISP) Violations

`UnifiedStorageEngine` forces engines to implement unnecessary methods:
```rust
impl UnifiedStorageEngine for SearchOnlyEngine {
    async fn do_flush(...) { unimplemented!() }     // Forced
    async fn do_compact(...) { unimplemented!() }   // Forced
    async fn search_vectors_unified(...) { ... }    // Only needed
}
```

### 5. Dependency Inversion Principle (DIP) Violations

- RL planner imports concrete engine paths (`viper_paths.rs`, `sst_paths.rs`)
- `UnifiedQueryOptimizer` directly instantiates engine cost models
- No abstraction for engine-specific cost calculation

---

## Proposed Architecture

### Core Design: Strategy Pattern + Capability Registry

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                          VectorSearchFramework                               │
├─────────────────────────────────────────────────────────────────────────────┤
│                                                                              │
│  ┌────────────────────┐    ┌────────────────────┐    ┌──────────────────┐   │
│  │ EngineCapabilities │◄───│   RL Planner       │───►│ ActionRegistry   │   │
│  │     Registry       │    │ (queries caps)     │    │ (unified space)  │   │
│  └─────────┬──────────┘    └────────────────────┘    └──────────────────┘   │
│            │                                                                 │
│            ▼                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │                     SearchStrategyChain                              │    │
│  │  ┌───────────────┐  ┌────────────────┐  ┌─────────────────────────┐ │    │
│  │  │ IndexStrategy │─►│ QuantStrategy  │─►│ PruningStrategy         │ │    │
│  │  │ (HNSW/IVF/...) │  │ (Progressive)  │  │ (Centroid/Sqrt/Ratio)  │ │    │
│  │  └───────────────┘  └────────────────┘  └─────────────────────────┘ │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│            │                                                                 │
│            ▼                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │                     UnifiedSearchExecutor                            │    │
│  │  - Orchestrates strategies                                           │    │
│  │  - Manages result processing                                         │    │
│  │  - Provides metrics/telemetry                                        │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│            │                                                                 │
│            ▼                                                                 │
│  ┌─────────────────────────────────────────────────────────────────────┐    │
│  │  SST    │  HELIX   │  VIPER   │  SWIFT   │  NOVA    │  RAPTOR       │    │
│  │ Reader  │  Reader  │  Reader  │  Reader  │  Reader  │  Reader       │    │
│  └─────────────────────────────────────────────────────────────────────┘    │
│                                                                              │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## New Trait Definitions

### 1. EngineCapabilities (Replaces hardcoded engine matching)

```rust
// File: src/storage/engine_capabilities.rs

/// Declares what an engine can do - queried by RL planner
pub trait EngineCapabilities: Send + Sync {
    /// Supported index types for this engine
    fn supported_indexes(&self) -> Vec<IndexType>;

    /// Supported quantization levels
    fn supported_quantization(&self) -> Vec<QuantizationType>;

    /// Supported pruning strategies
    fn supported_pruning(&self) -> Vec<PruningStrategy>;

    /// Supported search modes
    fn supported_search_modes(&self) -> Vec<SearchMode>;

    /// Whether engine supports AXIS index integration
    fn supports_axis_indexes(&self) -> bool;

    /// Whether engine supports predicate pushdown
    fn supports_predicate_pushdown(&self) -> bool;

    /// Cost model for this engine (for RL planner)
    fn cost_model(&self) -> &dyn EngineCostModel;

    /// Expected latency for collection size (for size-aware rewards)
    fn expected_latency(&self, collection_size: u64, top_k: usize) -> f64;
}

/// Cost model abstraction (DIP compliance)
pub trait EngineCostModel: Send + Sync {
    fn estimate_search_cost(&self, params: &SearchCostParams) -> CostEstimate;
    fn estimate_index_cost(&self, index_type: IndexType, collection_size: u64) -> CostEstimate;
}
```

### 2. SearchStrategy (SRP + Strategy Pattern)

```rust
// File: src/storage/search/strategy.rs

/// Single responsibility: Execute one type of search
pub trait SearchStrategy: Send + Sync {
    /// Check if this strategy can handle the given context
    fn can_handle(&self, ctx: &SearchContext) -> bool;

    /// Execute search and return candidates
    async fn execute(
        &self,
        ctx: &SearchContext,
        input: &SearchInput,
    ) -> Result<Vec<SearchCandidate>, SearchError>;

    /// Strategy priority (higher = preferred when multiple match)
    fn priority(&self) -> u32;

    /// Strategy name for logging/metrics
    fn name(&self) -> &'static str;
}

/// Concrete strategies implementing SearchStrategy
pub struct DirectScanStrategy { ... }
pub struct HnswIndexStrategy { ... }
pub struct IvfIndexStrategy { ... }
pub struct ProgressiveQuantStrategy { ... }
pub struct HilbertPruningStrategy { ... }
pub struct CentroidPruningStrategy { ... }
```

### 3. QuantizationPipeline (Replaces 4 duplicate implementations)

```rust
// File: src/compute/quantization/pipeline.rs

/// Unified progressive quantization pipeline
pub struct QuantizationPipeline {
    stages: Vec<Box<dyn QuantizationStage>>,
    distance_compute: Arc<UnifiedDistanceCompute>,
}

pub trait QuantizationStage: Send + Sync {
    /// Stage name (Binary, INT8, PQ4, PQ8, FP32)
    fn name(&self) -> &'static str;

    /// Bits per dimension
    fn bits_per_dim(&self) -> u8;

    /// Filter candidates using this quantization level
    async fn filter(
        &self,
        query: &[f32],
        candidates: Vec<SearchCandidate>,
        target_count: usize,
    ) -> Result<Vec<SearchCandidate>, QuantError>;
}

impl QuantizationPipeline {
    /// Execute progressive refinement: Binary → INT8 → PQ → FP32
    pub async fn execute_progressive(
        &self,
        query: &[f32],
        initial_candidates: Vec<SearchCandidate>,
        final_k: usize,
    ) -> Result<Vec<SearchCandidate>, QuantError> {
        let mut candidates = initial_candidates;

        for (i, stage) in self.stages.iter().enumerate() {
            let target = self.calculate_target_for_stage(i, final_k);
            candidates = stage.filter(query, candidates, target).await?;

            tracing::debug!(
                stage = stage.name(),
                input = initial_candidates.len(),
                output = candidates.len(),
                "Progressive stage completed"
            );
        }

        Ok(candidates)
    }
}
```

### 4. PruningStrategy (Unifies Centroid, Sqrt, Ratio, Hilbert)

```rust
// File: src/storage/search/pruning.rs

/// Block/file level pruning before full search
pub trait PruningStrategy: Send + Sync {
    /// Prune blocks based on query
    fn prune_blocks(
        &self,
        query: &[f32],
        blocks: &[BlockInfo],
        target_blocks: usize,
    ) -> Vec<&BlockInfo>;

    /// Strategy name
    fn name(&self) -> &'static str;
}

// Concrete implementations
pub struct SqrtPruning;           // Select sqrt(n) blocks
pub struct RatioPruning(f32);     // Select ratio% of blocks
pub struct CentroidPruning {      // Distance to block centroid
    threshold: f32,
}
pub struct HilbertPruning;        // Hilbert curve locality
pub struct ZoneMapPruning;        // Min/max bounds checking
```

### 5. SearchContext (Decomposed from StorageQueryContext)

```rust
// File: src/storage/search/context.rs

/// Minimal search input (ISP compliant)
pub struct SearchInput {
    pub query_vector: Vec<f32>,
    pub top_k: usize,
    pub distance_metric: DistanceMetric,
    pub filter: Option<FilterExpression>,
}

/// Collection context (separate concern)
pub struct CollectionContext {
    pub collection_id: String,
    pub storage_path: String,
    pub engine_type: StorageEngineType,
    pub collection_size: u64,
}

/// Search hints (optional optimization context)
pub struct SearchHints {
    pub preferred_index: Option<IndexType>,
    pub preferred_quantization: Option<QuantizationType>,
    pub target_recall: f32,
    pub max_latency_ms: Option<f64>,
}

/// Combined context (composed, not aggregated)
pub struct SearchContext {
    pub input: SearchInput,
    pub collection: CollectionContext,
    pub hints: Option<SearchHints>,
}
```

---

## UnifiedSearchExecutor Implementation

```rust
// File: src/storage/search/executor.rs

/// Central search orchestrator (replaces per-engine search logic)
pub struct UnifiedSearchExecutor {
    /// Available search strategies
    strategies: Vec<Arc<dyn SearchStrategy>>,

    /// Quantization pipeline
    quant_pipeline: Arc<QuantizationPipeline>,

    /// Pruning strategies
    pruning_strategies: HashMap<String, Arc<dyn PruningStrategy>>,

    /// Distance computation
    distance_compute: Arc<UnifiedDistanceCompute>,

    /// Result processor
    result_processor: ResultProcessor,

    /// Metrics collector
    metrics: Arc<SearchMetrics>,
}

impl UnifiedSearchExecutor {
    /// Execute search using best strategy
    pub async fn execute(
        &self,
        ctx: &SearchContext,
        capabilities: &dyn EngineCapabilities,
    ) -> Result<Vec<OptimizedSearchRecord>, SearchError> {
        let start = std::time::Instant::now();

        // 1. Select best strategy based on context + capabilities
        let strategy = self.select_strategy(ctx, capabilities)?;

        // 2. Apply pruning if enabled
        let pruned_scope = if let Some(pruning) = ctx.hints.as_ref()
            .and_then(|h| h.preferred_pruning.as_ref())
        {
            self.apply_pruning(ctx, pruning).await?
        } else {
            SearchScope::Full
        };

        // 3. Execute strategy
        let candidates = strategy.execute(ctx, &pruned_scope).await?;

        // 4. Apply progressive quantization if needed
        let refined = if self.should_use_progressive_quant(ctx, capabilities) {
            self.quant_pipeline.execute_progressive(
                &ctx.input.query_vector,
                candidates,
                ctx.input.top_k,
            ).await?
        } else {
            candidates
        };

        // 5. Process results (dedupe, rank, format)
        let results = self.result_processor.process(refined, ctx.input.top_k);

        // 6. Record metrics
        self.metrics.record_search(
            strategy.name(),
            start.elapsed(),
            results.len(),
            ctx.collection.collection_size,
        );

        Ok(results)
    }

    fn select_strategy(
        &self,
        ctx: &SearchContext,
        capabilities: &dyn EngineCapabilities,
    ) -> Result<&dyn SearchStrategy, SearchError> {
        // Filter strategies by capability support
        let viable: Vec<_> = self.strategies.iter()
            .filter(|s| s.can_handle(ctx))
            .filter(|s| self.strategy_supported_by_engine(s, capabilities))
            .collect();

        // Select highest priority
        viable.into_iter()
            .max_by_key(|s| s.priority())
            .map(|s| s.as_ref())
            .ok_or(SearchError::NoViableStrategy)
    }
}
```

---

## RL Planner Integration (DIP Compliant)

```rust
// File: src/query/rl_planner/capability_aware.rs

/// RL planner that queries engine capabilities (no hardcoding)
pub struct CapabilityAwareRLPlanner {
    /// Thompson Sampling bandit
    bandit: ContextualBandit,

    /// Reward calculator with size-aware scaling
    reward_calc: RewardCalculator,
}

impl CapabilityAwareRLPlanner {
    /// Select action based on capabilities (not hardcoded paths)
    pub fn select_action(
        &self,
        state: &PlannerState,
        capabilities: &dyn EngineCapabilities,
    ) -> ExecutionAction {
        // Build action space dynamically from capabilities
        let action_space = self.build_action_space(capabilities);

        // Sample using Thompson Sampling
        let action_id = self.bandit.sample(&state, &action_space);

        action_space.get_action(action_id)
    }

    fn build_action_space(&self, caps: &dyn EngineCapabilities) -> ActionSpace {
        let mut space = ActionSpace::new();

        // Add index actions based on capability
        for index in caps.supported_indexes() {
            space.add_index_action(index);
        }

        // Add quantization actions
        for quant in caps.supported_quantization() {
            space.add_quant_action(quant);
        }

        // Add pruning actions
        for prune in caps.supported_pruning() {
            space.add_prune_action(prune);
        }

        // Add search mode actions
        for mode in caps.supported_search_modes() {
            space.add_mode_action(mode);
        }

        space
    }

    /// Calculate reward using size-aware model from capabilities
    pub fn calculate_reward(
        &self,
        result: &ExecutionResult,
        capabilities: &dyn EngineCapabilities,
    ) -> f32 {
        let expected = capabilities.expected_latency(
            result.collection_size,
            result.top_k,
        );

        self.reward_calc.calculate_with_collection_size(
            result.latency_ms,
            result.recall,
            result.throughput,
            result.collection_size,
            Some(expected),
        )
    }
}
```

---

## Migration Path

### Phase 1: Extract Core Abstractions (Week 1-2)

| Task | Files | Impact |
|------|-------|--------|
| Create `EngineCapabilities` trait | `src/storage/engine_capabilities.rs` | Enables DIP |
| Create `SearchStrategy` trait | `src/storage/search/strategy.rs` | Enables SRP |
| Create `QuantizationPipeline` | `src/compute/quantization/pipeline.rs` | Eliminates 600 LOC |
| Create `PruningStrategy` trait | `src/storage/search/pruning.rs` | Unifies pruning |

### Phase 2: Implement Concrete Strategies (Week 2-3)

| Strategy | Migrated From | LOC Saved |
|----------|---------------|-----------|
| `DirectScanStrategy` | All 6 engines | ~200 |
| `HnswIndexStrategy` | SST, HELIX, SWIFT | ~150 |
| `IvfIndexStrategy` | SST, NOVA | ~100 |
| `ProgressiveQuantStrategy` | HELIX, SWIFT, NOVA, RAPTOR | ~600 |
| `CentroidPruningStrategy` | SST, RAPTOR | ~150 |
| `HilbertPruningStrategy` | HELIX | ~100 |

### Phase 3: Migrate Engines (Week 3-4)

| Engine | Changes | Complexity |
|--------|---------|------------|
| SST | Implement `EngineCapabilities`, use `UnifiedSearchExecutor` | Medium |
| HELIX | Same + migrate Hilbert pruning | Medium |
| VIPER | Same + predicate pushdown capability | Low |
| SWIFT | Same + in-memory optimization | Low |
| NOVA | Same + zone map capability | Medium |
| RAPTOR | Same + tier-aware capability | High |

### Phase 4: RL Planner Migration (Week 4-5)

| Task | Current | Target |
|------|---------|--------|
| Delete engine-specific paths | 6 files | 0 files |
| Implement `CapabilityAwareRLPlanner` | Hardcoded | Dynamic |
| Update `UnifiedQueryOptimizer` | Direct instantiation | Capability queries |

---

## Expected Benefits

### Quantitative

| Metric | Before | After | Improvement |
|--------|--------|-------|-------------|
| Total search code | 2,900+ LOC | 1,100 LOC | 62% reduction |
| Progressive search impls | 4 copies | 1 unified | 75% reduction |
| RL planner config files | 6 files | 1 file | 83% reduction |
| Test surface area | High | Low | 3x faster tests |

### Qualitative

1. **Natural Performance Improvement**: When one strategy improves, all engines benefit
2. **Easier Experimentation**: Add new strategies without touching engines
3. **Better RL Learning**: Unified action space enables cross-engine learning
4. **Clearer Responsibility**: Each component does one thing well
5. **Simpler Debugging**: Centralized metrics and logging

---

## File Changes Summary

### New Files to Create

```
src/storage/
├── engine_capabilities.rs          # EngineCapabilities trait
├── search/
│   ├── mod.rs                       # Module exports
│   ├── strategy.rs                  # SearchStrategy trait + impls
│   ├── pruning.rs                   # PruningStrategy trait + impls
│   ├── executor.rs                  # UnifiedSearchExecutor
│   ├── context.rs                   # SearchInput, SearchContext
│   └── result_processor.rs          # Result dedup/ranking
│
src/compute/quantization/
├── pipeline.rs                      # QuantizationPipeline
├── stages/
│   ├── mod.rs
│   ├── binary_stage.rs
│   ├── int8_stage.rs
│   ├── pq_stage.rs
│   └── fp32_stage.rs
│
src/query/rl_planner/
├── capability_aware.rs              # CapabilityAwareRLPlanner
├── action_space.rs                  # Dynamic action space builder
```

### Files to Modify

```
src/storage/traits.rs                # Split into focused traits
src/storage/engines/impls/*/mod.rs   # Implement EngineCapabilities
src/query/unified_query_optimizer.rs # Use capability queries
```

### Files to Delete (after migration)

```
src/query/rl_planner/paths/
├── sst_paths.rs
├── helix_paths.rs
├── viper_paths.rs
├── swift_paths.rs
├── nova_paths.rs
└── raptor_paths.rs

src/storage/engines/impls/*/progressive_search.rs  # Duplicates
```

---

## Validation Criteria

After implementation, the following should pass:

1. **All existing tests pass** with no behavior change
2. **Benchmark shows equal or better performance** for all engines
3. **RL planner dynamically adapts** to engine capabilities
4. **New strategy can be added** without modifying engines
5. **Code coverage improves** due to centralized logic

---

## Appendix: Benchmark Evidence

From `/tmp/all_engines_benchmark.log`, the RL planner already explores diverse strategies:

```
SST 2K:    DirectScan, Exact, FP32                    → reward=0.867
SST 10K:   HNSW(m=16, ef=50), Approximate, FP32       → reward=0.786
HELIX 2K:  HNSW(m=16, ef=100), Approximate, FP32      → reward=0.897
HELIX 10K: DirectScan, Approximate, Binary→INT8→PQ8→FP32 → reward=0.739
VIPER 2K:  DirectScan, Exact, Ratio(0.5)              → reward=0.829
SWIFT 2K:  DirectScan, Approximate, Binary→INT8→FP32  → reward=0.915
NOVA 2K:   IVF(nprobe=16), Approximate, FP32          → reward=0.850
RAPTOR 2K: DirectScan, Exact, Sqrt pruning            → reward=0.936
```

The unified framework enables sharing these successful strategies across engines that currently can't use them.
