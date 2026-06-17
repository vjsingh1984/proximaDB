# ProximaDB Ranking Framework — Spec, HLD, LLD

| Field | Value |
|---|---|
| **Status** | Draft (R-0 landed, R-1 next) |
| **Owner** | TBD |
| **Created** | 2026-05-23 |
| **Last updated** | 2026-05-23 |
| **Supersedes** | n/a — net-new initiative |
| **Related** | `roadmap/MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc`, `docs/12-design/COMPETITIVE_OLTP_OLAP_MPP_TRAJECTORY_2026_05_20.adoc`, `docs/12-design/adr/ADR-004-unified-explain-contract.adoc` |
| **Reviewers** | TBD |

---

## 0. TL;DR

ProximaDB has solid retrieval (HNSW/IVF/PQ/Annoy/LSH/DiskANN), production hybrid fusion (RRF, weighted, RBP, Borda, MMR), an in-process ONNX embedding service (`ort`-backed BGE small/large/m3), and a cross-modal reranker with explanations. It lacks a **multi-phase ranking framework** — the equivalent of Vespa's FEF / `MatchMaster` / `MatchThread` / tensor framework / ONNX-at-serving — and lacks **in-process cross-encoder inference for scoring** (the existing `ort` integration only generates embeddings).

This document specs a multi-phase ranking framework that:

1. Closes the gap with **Vespa** (multi-phase, feature DAG, expression evaluation, ONNX models in the scoring path).
2. Closes the gap with **Weaviate** (apply rerank *after* RRF fusion, not as a parallel plugin).
3. Reuses what proximaDB already ships: `ort` (extracted into a shared scorer-session primitive), `CrossModalReranker` (adopted as a global-phase executor), hybrid fusion (becomes an upstream operator), RL planner (gains a new action dimension), `ScoreComponent` (promoted from the reranker crate into `core::search`).
4. Stays inside the CLAUDE.md architectural mandates: reuse-first, no parallel implementations, canonical `ProximaRecord`/`ProximaValue`, xCatalog as durable authority, RLS-aware, observability + explainability first-class.

The shape is **`Blueprint` → `BlueprintResolver` → `FeatureExecutor` + `LazyValue`** (Vespa's pattern, in Rust), composed into **first / second / global phases** with arena-allocated outputs, lazy memoization per doc, and a small interpreted expression VM (optional Cranelift JIT in a later phase).

---

## 1. Gap analysis

### 1.1 What proximaDB ships today (must reuse, must not duplicate)

| Capability | Location | State |
|---|---|---|
| Fusion strategies — RRF, weighted, RBP, Borda, normalized, Dempster-Shafer | `src/core/search/hybrid/mod.rs:55-200`, `crates/query/proximadb-query-fusion/src/lib.rs` | Production |
| Cross-modal reranker w/ MMR, intent, temporal decay, explanations | `crates/query/proximadb-query/src/reranking.rs:1-1250` | Production |
| `ScoreComponent { name, value, weight, contribution }` (`f64`) | `crates/query/proximadb-query/src/reranking.rs:481-492` | Production — but trapped inside the reranker crate |
| `RerankedResult { records, explanations, quality_score, diversity_score }` | `reranking.rs:494-505` | Production |
| In-process ONNX runtime via `ort` (mmap'd sessions, tier-aware routing) | `crates/modalities/proximadb-embedding/src/models/mod.rs:48-120` | Production — embeddings only |
| Tokenizers (`SharedTokenizer`) | `crates/modalities/proximadb-embedding/src/tokenizer.rs` | Production |
| Cost-aware RL routing planner (Thompson Sampling) | `src/query/rl_planner/mod.rs` | Production — routing only, not scoring |
| Unified executor (multi-model result envelope) | `src/query/unified/executor.rs` | Production |
| Hybrid coordinator (parallel BM25 + vector + fusion) | `src/core/search/hybrid/coordinator.rs` | Production |
| `OptimizedSearchRecord` (modality-tagged record envelope) | `src/core/search/results.rs:204-294` | Production — flat `score: f32`, no components |

### 1.2 What's missing (the gap, with Vespa/Weaviate analogues)

| Missing capability | Vespa | Weaviate |
|---|---|---|
| Multi-phase ranking (first / second / global) | `MatchMaster` + `MatchThread` + `MatchLoopCommunicator` | Plugin runs once, post-retrieval, before pagination |
| Feature DAG with lazy memoized per-doc eval | `Blueprint` + `BlueprintResolver` + `FeatureExecutor` + `LazyValue` | None — opaque external call |
| Rank-profile DSL (declarative, inheritable, per-collection) | `.sd` `rank-profile`, `RankProfile.java` | Per-class `moduleConfig` (opaque map) |
| **In-process cross-encoder ONNX inference for scoring** | `eval/onnx/`, `OnnxModelCache` | All rerankers are HTTP-out (Cohere, Voyage, Jina, …) |
| Expression compilation (interpreted bytecode minimum) | `TensorFunction` → `InterpretedFunction` / LLVM `CompiledFunction` | None |
| Rerank-after-fusion composition (the explicit Weaviate gap) | Phase ordering inside `match_master.cpp` | **Explicitly disabled** (see `explorer_hybrid.go:400` comment) |
| Plugin/hook surface in the read path | `BlueprintFactory` registration | Module trait per type |
| **Match-features** / **summary-features** (debug + training export) | First-class | None |
| Per-tenant model registry with hot-reload + LRU eviction | `OnnxModelCache::Token` (refcounted) | n/a — external services |

### 1.3 Evidence — Weaviate's explicit fusion-gap

> `usecases/traverser/explorer_hybrid.go:~400`
> *"This operation cannot be performed with hybrid search. In case of hybrid it needs to be done later with combined results from vector and keyword search"*

The hybrid pipeline runs (dense || sparse) → RRF/score-fusion → postProcess → grouping → pagination — with no reranker hook between fusion and pagination. ProximaDB will avoid this by treating fusion as an upstream node and ranking as a downstream pipeline.

---

## 2. Requirements

### 2.1 Functional (FR)

| ID | Requirement | Source |
|---|---|---|
| FR-1 | N-phase ranking with at least 3 phases: `first_phase` (per-doc, all candidates), `second_phase` (rescore top-K per shard), `global_phase` (rescore top-K post-merge) | Vespa parity |
| FR-2 | Declarative **rank profile** DSL attachable to a collection, inheritable, overridable per-query, persisted in xCatalog with version semantics | Vespa parity |
| FR-3 | **Feature DAG** with named features (`bm25`, `cosine(query, field)`, `freshness`, `model(rerank_v3)`, `attribute(x)`, user functions). Topological eval, per-doc memoization | Vespa parity |
| FR-4 | **In-process ONNX cross-encoder** inference. Reuses `ort` from embedding crate via a shared `ScorerSession` primitive | New capability |
| FR-5 | **Expression evaluator** for scalar rank expressions. v1 interpreted bytecode; v2 optional Cranelift JIT | Vespa parity |
| FR-6 | Phase ordering composes with hybrid: `(vector + bm25) → fusion → first_phase → second_phase → global_phase` | Weaviate gap fix |
| FR-7 | **Match-features** + **summary-features** in result payload for debugging and offline training pipelines | Vespa parity |
| FR-8 | Per-tenant **model registry**: ONNX/tokenizer artifacts referenced by id, versioned, lazily loaded, hot-swappable, LRU evictable | New |
| FR-9 | Surfaced via REST, gRPC, Arrow Flight (`feature_batch` action), **pgwire** (`RERANK(...)` SQL function) | Multi-protocol mandate |
| FR-10 | RL planner gains a new action dimension: `RankPhaseChoice` (skip second, skip global, second-phase k, batch size) | Reuse existing optimizer |
| FR-11 | Profile mutations persisted through canonical WAL/manifest path; RLS-scoped | CLAUDE.md mandate |
| FR-12 | EXPLAIN integration: rank pipeline shape must be visible in unified EXPLAIN contract (ADR-004) | Architecture mandate |
| FR-13 | **Backwards compatibility**: collections without an attached profile behave identically to today (zero-cost abstraction) | NFR-9 below |
| FR-14 | Phase budget enforcement: per-phase wall-clock + CPU limits; on exceed, return prior-phase order with `phase_truncated=true` flag | Tail latency control |

### 2.2 Non-functional (NFR)

| ID | Requirement | Target |
|---|---|---|
| NFR-1 | First-phase per-doc cost (single feature) | ≤ 50 ns / doc |
| NFR-2 | First-phase per-doc cost (5-feature expression) | ≤ 250 ns / doc |
| NFR-3 | Second-phase cross-encoder, batch=32, 384-dim, INT8 model | ≤ 30 ms p95 |
| NFR-4 | Zero allocations on the per-doc hot path (arena-allocated feature outputs) | enforced by Miri / allocator test |
| NFR-5 | Model session memory reuse: one `ort::Session` per `(model_id, version)` per node | enforced by `model_registry` test |
| NFR-6 | Thread-safe concurrent queries: `Arc<RankProgram>` cloned cheaply across worker threads | enforced by stress test |
| NFR-7 | Hot-reload of rank profiles via xCatalog watcher: ≤ 10 ms cutover, no in-flight query disruption | RCU pattern test |
| NFR-8 | Per-feature observability: histogram `rank_feature_latency_us{profile,phase,feature}` and gauge `rank_feature_contribution{...}` | wired into existing Prom registry |
| NFR-9 | **Zero-cost when no profile attached**: no profile → identical hot path to today | regression bench gate |
| NFR-10 | Model cold load | ≤ 800 ms |
| NFR-11 | Global-phase budget | tunable; default 100 ms p95 |
| NFR-12 | Profile cache: ≥ 99 % hit rate under steady state | per-profile counter |

### 2.3 Out of scope (deferred or explicitly NOT)

| Item | Why deferred |
|---|---|
| Full **tensor expression DSL** with named dimensions (Vespa-style) | R-9 optional — only if user pull is concrete |
| Distributed training (model training inside ProximaDB) | Inference-only system; training stays external |
| LLM-based listwise reranking via remote APIs (OpenAI, Cohere rerank-v3) | Phase-3 optional adapter; not core |
| GPU inference for cross-encoders | Roadmap follow-up; reuse existing Metal feature gate |
| Tensor framework as a first-class type system in `ProximaValue` | Future; would touch the data model |
| Reinforcement-learned feature weights | Roadmap — depends on training-data export landing first |

---

## 3. High-Level Design

### 3.1 Architecture diagram

```
                  ┌─────────────────────────────────────────────────────────────┐
                  │  Query Handler  (REST / gRPC / Arrow Flight / pgwire)       │
                  │                                                              │
                  │  Parses QueryRequest { rank_profile_id, rank_overrides }    │
                  └────────────┬────────────────────────────────────────────────┘
                               │
                               ▼
                  ┌─────────────────────────────────────┐
                  │ Query Optimizer + RL Planner         │
                  │   chooses retrieval plan AND         │  <- new joint action
                  │   RankPhaseChoice                    │
                  └────────────┬────────────────────────┘
                               │
                ┌──────────────┴──────────────┐
                ▼                             ▼
   ┌─────────────────────────┐    ┌─────────────────────────┐
   │ Vector retrieval        │    │ BM25 retrieval          │
   │ (HNSW, IVF, …)          │    │ (Tantivy / sparse)      │
   └──────────┬──────────────┘    └─────────┬───────────────┘
              │                              │
              └──────────────┬───────────────┘
                             ▼
            ┌──────────────────────────────────┐
            │ Fusion (RRF / weighted / …)       │   <- existing hybrid module
            └──────────────┬────────────────────┘
                           │  CandidateStream
                           ▼
            ┌──────────────────────────────────┐
            │ FIRST PHASE  (per-shard, per-thread)
            │  - cheap features                 │
            │  - per-doc DAG eval               │
            │  - heap keeps top H               │
            └──────────────┬────────────────────┘
                           │  PartialHits (size = H)
                           ▼
            ┌──────────────────────────────────┐
            │ SECOND PHASE  (per-shard)         │
            │  - rescore top K  (K ≤ H)         │
            │  - cross-encoder ONNX, MMR, …     │
            │  - batched model calls            │
            └──────────────┬────────────────────┘
                           │  ShardHits (size = K)
                           ▼
            ┌──────────────────────────────────┐
            │ MERGE (cross-shard, async)        │
            └──────────────┬────────────────────┘
                           │  MergedHits (size = K · shards)
                           ▼
            ┌──────────────────────────────────┐
            │ GLOBAL PHASE                      │
            │  - cross-modal reranker (existing)│
            │  - LLM listwise (optional)        │
            └──────────────┬────────────────────┘
                           ▼
                ScoreVector { primary, components } + match_features + summary_features
                           ▼
                  Response serialization
```

### 3.2 Workspace impact

```
crates/
├── ranking/                                    # NEW top-level group
│   ├── proximadb-rank-core/
│   │   └── src/{lib,blueprint,executor,program,pipeline,arena,error,context,types}.rs
│   ├── proximadb-rank-expr/
│   │   └── src/{lib,grammar,parser,ast,bytecode,vm,types,builtins}.rs
│   ├── proximadb-rank-onnx/
│   │   └── src/{lib,scorer_session,model_cache,wire_plan,batch}.rs
│   ├── proximadb-rank-features/
│   │   └── src/{lib,attribute,closeness,bm25,freshness,decay,model_feat,expr_feat,cross_modal}.rs
│   └── proximadb-rank-profile/
│       └── src/{lib,dsl,schema,xcatalog_binding,validator,inheritance}.rs
├── modalities/proximadb-embedding/             # MODIFY — extract shared ScorerSession into rank-onnx
└── query/proximadb-query/                      # MODIFY — CrossModalReranker becomes a GlobalScorer
src/
├── core/search/
│   ├── results.rs                              # MODIFY — add ScoreVector
│   └── rank/                                   # NEW — integration glue
│       └── mod.rs (RankPipelineBuilder, profile lookup, phase orchestration)
├── query/rl_planner/                           # MODIFY — add RankPhaseChoice action
└── services/model_registry/                    # NEW — lifecycle + LRU eviction
```

This respects the CLAUDE.md workspace map (`Foundation → Contracts → Modality Runtime → Cross-Model Query Runtime → Platform Runtime → Apps/Bindings`):

- `proximadb-rank-*` crates are **Modality Runtime** (alongside `proximadb-embedding`).
- `model_registry` is **Platform Runtime**.
- `core::search::rank` is the **Cross-Model Query Runtime** glue.

### 3.3 Design patterns adopted

| Pattern | Source | Use site |
|---|---|---|
| **Blueprint → Executor** dual-mode | Vespa | `rank-core::blueprint` |
| **Lazy memoization per doc** via `LazyValue` | Vespa | `rank-core::program` |
| **Arena-allocated feature outputs** (`bumpalo::Bump`) | Custom | `rank-core::arena` |
| **Refcounted session tokens** (`Arc<ScorerSession>`) | Vespa `OnnxModelCache::Token` | `rank-onnx::model_cache` |
| **Phase budget guard** (deadline-bounded execution) | New | `rank-core::pipeline` |
| **RCU (read-copy-update) for profile hot-reload** | Standard | `rank-profile::xcatalog_binding` |
| **Factory registry** (`BlueprintFactory`) | Mirrors `storage::engines::factory` | `rank-features::lib` |
| **Score vector with components** | Vespa match-features | `core::search::results` |
| **Single optimizer extension** (no parallel planner) | CLAUDE.md mandate | `query::rl_planner` |
| **Canonical types** (`ProximaRecord`, `ProximaValue`) carried through | CLAUDE.md mandate | All interfaces |

### 3.4 Integration boundaries

| System | How ranking touches it |
|---|---|
| **xCatalog** | `RankProfile` is a first-class catalog resource. CRUD via existing catalog APIs. Versioned. RLS-scoped. |
| **WAL/manifest** | Profile mutations go through canonical WAL. Model artifacts are content-addressed blobs in object storage; only the manifest entry (model_id, version, sha256, uri, size, dtype, dims) is WAL-logged. |
| **Hybrid module** | `HybridCoordinator` becomes one of several upstream `CandidateSource`s feeding the pipeline. No code deletion; `CrossModalReranker.rerank()` adapts to `GlobalScorer::score()`. |
| **RL planner** | New `RankPhaseChoice` joins `(retrieval_plan, rank_phase_choice)` action tuple. Same Thompson Sampling, larger action space, new reward feature: `(latency_ms, quality_estimate)`. |
| **EXPLAIN** | `RankPipeline::explain()` emits a node into the unified EXPLAIN tree (ADR-004). Phase nodes carry features used + budget vs actual. |
| **Observability** | `rank_phase_latency_us{profile,phase}`, `rank_feature_latency_us{profile,phase,feature}`, `rank_feature_contribution{...}`, `rank_phase_truncated_total{profile,phase}`, `rank_model_cache_hit_ratio{model_id}`, `rank_model_evictions_total{model_id}`. |
| **Multi-protocol** | All four protocols share one `RankRequest` proto type. pgwire exposes `RERANK(collection, query_vec, k, profile)` SRF. |
| **CDC** | Profile changes emit catalog-change CDC events through the existing connector path. No new sink type. |

---

## 4. Low-Level Design

### 4.1 Crate `proximadb-rank-core`

#### 4.1.1 `error.rs`

```rust
#[derive(Debug, thiserror::Error)]
pub enum RankError {
    #[error("rank profile not found: {0}")]
    ProfileNotFound(String),
    #[error("rank profile validation failed: {0}")]
    InvalidProfile(String),
    #[error("feature not registered: {0}")]
    UnknownFeature(String),
    #[error("feature dependency cycle through {0}")]
    DependencyCycle(String),
    #[error("feature dependency depth exceeded max {max}")]
    DependencyTooDeep { max: usize },
    #[error("expression parse error: {0}")]
    ExpressionParse(String),
    #[error("expression type error: {0}")]
    ExpressionType(String),
    #[error("phase budget exceeded: {phase:?} after {elapsed_us}us (budget {budget_us}us)")]
    PhaseBudgetExceeded { phase: PhaseId, elapsed_us: u64, budget_us: u64 },
    #[error("model load failed: {model_id}: {source}")]
    ModelLoad { model_id: String, source: Box<dyn std::error::Error + Send + Sync> },
    #[error("model inference failed: {model_id}: {reason}")]
    ModelInference { model_id: String, reason: String },
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
pub type RankResult<T> = Result<T, RankError>;
```

`RankError` converts to `ProximaDBError::RankError` via a `#[from]` arm added to `core::errors::core_error`.

#### 4.1.2 `types.rs`

```rust
/// Stable id for a phase. 0 = first, 1 = second, 2 = global.
#[derive(Copy, Clone, Eq, PartialEq, Debug, serde::Serialize, serde::Deserialize)]
pub struct PhaseId(pub u8);

/// Index of an executor in a RankProgram (post-resolution).
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct ExecutorIdx(pub u16);

/// {executor_index, output_index} — O(1) wiring after resolution.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct FeatureRef {
    pub executor: ExecutorIdx,
    pub output: u8,
}

/// Output slot. Tagged union; the tag is implicit per executor's declared type.
#[repr(C)]
pub union OutputSlot {
    pub f: f32,
    pub tensor: std::mem::ManuallyDrop<TensorRef>,
}

/// Arena-allocated tensor view; lifetime tied to RankProgram::arena.
#[derive(Copy, Clone)]
pub struct TensorRef {
    pub ptr: *const f32,
    pub shape: [u16; 4],
    pub rank: u8,
}

/// Handle to a document within the current scoring context.
/// Internally a u32 doc-id local to the segment being scored.
#[derive(Copy, Clone, Eq, PartialEq, Debug)]
pub struct DocHandle(pub u32);
```

#### 4.1.3 `arena.rs`

```rust
pub struct FeatureArena {
    bump: bumpalo::Bump,
    high_water: std::cell::Cell<usize>,   // for observability
}

impl FeatureArena {
    pub fn new() -> Self { … }
    pub fn alloc_tensor(&self, shape: &[u16], data: &[f32]) -> TensorRef { … }
    pub fn reset(&mut self) { … }   // O(1)
    pub fn high_water_bytes(&self) -> usize { self.high_water.get() }
}
```

Tensor allocations live in the arena. The arena resets per-doc in first phase and per-batch in second phase, ensuring the per-doc hot path is allocation-free in steady state (NFR-4).

#### 4.1.4 `blueprint.rs`

```rust
/// Schema-time prototype + query-time configured instance.
pub trait Blueprint: Send + Sync + 'static {
    fn name(&self) -> &str;
    fn declared_inputs(&self) -> &[InputSpec];
    fn declared_outputs(&self) -> &[OutputSpec];
    fn build_executor(
        &self,
        cfg: &PhaseConfig,
        query_ctx: &QueryContext,
    ) -> RankResult<Box<dyn FeatureExecutor>>;
}

pub struct InputSpec { pub name: String, pub kind: ValueKind }
pub struct OutputSpec { pub name: String, pub kind: ValueKind }
pub enum ValueKind { F32, Tensor { rank: u8 } }

/// Registry of blueprints keyed by name. Mirrors storage::engines::factory shape.
pub struct BlueprintFactory {
    inner: dashmap::DashMap<String, Arc<dyn Blueprint>>,
}
impl BlueprintFactory {
    pub fn register(&self, bp: Arc<dyn Blueprint>);
    pub fn lookup(&self, name: &str) -> Option<Arc<dyn Blueprint>>;
    pub fn registered_names(&self) -> Vec<String>;
}
```

#### 4.1.5 `executor.rs`

```rust
pub trait FeatureExecutor: Send {
    /// Bound once per query. Inputs are LazyValue refs into the program's outputs vec.
    fn bind(&mut self, inputs: &[LazyValue], outputs: &mut [OutputSlot]);

    /// Compute outputs for one doc. Only invoked when a downstream LazyValue is forced.
    fn execute(&mut self, doc: DocHandle, ctx: &mut ScoreCtx);

    /// Optional: pre-compute constants once per query (before any doc is scored).
    fn precompute(&mut self, _ctx: &mut ScoreCtx) {}

    /// Optional: flush batched work at end of phase (used by ONNX scorers).
    fn end_of_phase(&mut self, _ctx: &mut ScoreCtx) -> RankResult<()> { Ok(()) }
}

/// Lazy-evaluated reference into a program output slot.
/// `executor: None` means constant (always ready); `Some(idx)` means force via execute.
#[derive(Copy, Clone)]
pub struct LazyValue<'a> {
    slot: *const OutputSlot,
    executor: Option<ExecutorIdx>,
    program: *mut RankProgram, // for forcing
    _phantom: std::marker::PhantomData<&'a OutputSlot>,
}

impl<'a> LazyValue<'a> {
    #[inline(always)]
    pub fn as_f32(self, doc: DocHandle, ctx: &mut ScoreCtx) -> f32 {
        if let Some(idx) = self.executor {
            // SAFETY: program is borrowed mutably for the duration of the per-doc call;
            // executor's outputs do not alias inputs.
            unsafe { (*self.program).force(idx, doc, ctx); }
        }
        unsafe { (*self.slot).f }
    }

    pub fn as_tensor(self, doc: DocHandle, ctx: &mut ScoreCtx) -> TensorRef { … }
}
```

The `unsafe` block is local and bounded; the program runs single-threaded per shard worker (each thread owns its own `RankProgram` clone), so there's no aliasing across threads. Borrow checker can't express "I own this for the duration of one call" cleanly here without significant indirection. The alternative (interior mutability via `RefCell`) costs a per-access check on the hot path — measured cost is ~3ns / access which violates NFR-1 budget.

#### 4.1.6 `program.rs`

```rust
pub struct RankProgram {
    executors:     SmallVec<[Box<dyn FeatureExecutor>; 16]>,
    outputs:       Vec<OutputSlot>,                    // flat, indexed by executor's output offset
    output_offsets: SmallVec<[u16; 16]>,               // executors[i].outputs = &outputs[offsets[i]..offsets[i+1]]
    score_feature: FeatureRef,                         // root for this program
    last_doc:      std::cell::Cell<Option<DocHandle>>, // memoization watermark
    forced_bitmap: bit_vec::BitVec,                    // which executors have run for `last_doc`
    arena:         FeatureArena,
    profile_id:    String,
    phase:         PhaseId,
}

impl RankProgram {
    /// Topologically pre-compute constants once per query.
    pub fn setup(&mut self, ctx: &mut ScoreCtx) -> RankResult<()> { … }

    /// Score a single doc. Hot path.
    #[inline(always)]
    pub fn rank(&mut self, doc: DocHandle, ctx: &mut ScoreCtx) -> f32 {
        if self.last_doc.get() != Some(doc) {
            self.last_doc.set(Some(doc));
            self.forced_bitmap.clear();
            self.arena.reset();
        }
        let lv = self.root_lazy_value();
        lv.as_f32(doc, ctx)
    }

    /// Internal — called by LazyValue::as_f32 when slot needs filling.
    fn force(&mut self, exec: ExecutorIdx, doc: DocHandle, ctx: &mut ScoreCtx) {
        if !self.forced_bitmap.get(exec.0 as usize).unwrap_or(false) {
            self.executors[exec.0 as usize].execute(doc, ctx);
            self.forced_bitmap.set(exec.0 as usize, true);
        }
    }

    pub fn end_of_phase(&mut self, ctx: &mut ScoreCtx) -> RankResult<()> {
        for ex in &mut self.executors { ex.end_of_phase(ctx)?; }
        Ok(())
    }
}
```

#### 4.1.7 `pipeline.rs`

```rust
pub struct RankPipeline {
    profile_id: String,
    first:      Arc<RankProgramTemplate>,             // template; per-worker .build()
    second:     Option<Arc<RankProgramTemplate>>,
    global:     Option<Arc<dyn GlobalScorer>>,
    budget:     PhaseBudget,
    match_feats: Vec<FeatureRef>,                     // emitted in payload
    summary_feats: Vec<FeatureRef>,                   // emitted in summary
}

#[derive(Clone, Debug)]
pub struct PhaseBudget {
    pub first_max_us:  Option<u64>,
    pub second_max_us: Option<u64>,
    pub global_max_us: Option<u64>,
}

pub trait GlobalScorer: Send + Sync {
    fn score<'a>(
        &'a self,
        hits: MergedHits,
        topk: usize,
        ctx: &'a mut ScoreCtx,
    ) -> futures::future::BoxFuture<'a, RankResult<RankedBatch>>;
}

impl RankPipeline {
    pub async fn run(
        &self,
        candidates: CandidateStream,
        topk: usize,
        ctx: &mut ScoreCtx,
    ) -> RankResult<RankedBatch> {
        let heap_size = self.first.heap_size.unwrap_or(topk * 4);
        let phase1 = self.run_first_phase(candidates, heap_size, ctx)?;
        let phase2 = if let Some(tpl) = &self.second {
            self.run_second_phase(tpl, phase1, tpl.rerank_count.unwrap_or(topk * 2), ctx)?
        } else { phase1 };
        let phase3 = if let Some(g) = &self.global {
            g.score(phase2.merged(), topk, ctx).await?
        } else { phase2.truncate(topk) };
        Ok(phase3.with_features(&self.match_feats, &self.summary_feats))
    }

    fn explain(&self) -> ExplainNode { … } // for ADR-004
}
```

#### 4.1.8 `context.rs`

```rust
/// Per-query, per-thread mutable scoring context.
/// Carries access to candidates, model cache, query vectors, attribute accessors,
/// observability sink, and the deadline.
pub struct ScoreCtx<'a> {
    pub query: &'a QueryContext,
    pub deadline: Option<std::time::Instant>,
    pub model_cache: &'a dyn ModelCache,
    pub attribute_access: &'a dyn AttributeAccess,   // column store reader
    pub candidate_data: &'a dyn CandidateData,       // per-candidate retrieval metadata
    pub batch: &'a mut BatchScratch,                 // for cross-encoder accumulation
    pub metrics: &'a dyn RankMetricsSink,
}
```

### 4.2 Crate `proximadb-rank-expr`

#### 4.2.1 Grammar (PEG)

```
expr      <- add
add       <- mul (('+' / '-') mul)*
mul       <- unary (('*' / '/') unary)*
unary     <- '-' unary / pow
pow       <- atom ('^' unary)?
atom      <- number / call / feature / paren / if_expr
paren     <- '(' expr ')'
if_expr   <- 'if' '(' expr ',' expr ',' expr ')'
call      <- ident '(' args? ')'
feature   <- ident ('(' literal_args? ')')?
args      <- expr (',' expr)*
literal_args <- literal (',' literal)*
literal   <- string / number
ident     <- [a-zA-Z_][a-zA-Z0-9_]*
number    <- '-'? digit+ ('.' digit+)? (('e'/'E') '-'? digit+)?
string    <- '"' [^"]* '"' / "'" [^']* "'"
```

Built-in functions: `max`, `min`, `if`, `log`, `exp`, `pow`, `sqrt`, `sigmoid`, `relu`, `tanh`, `clamp`, `abs`.

Feature references like `bm25(title)`, `closeness(embedding)`, `attribute(price)`, `model("rerank-v3")` are resolved by the DAG resolver against the `BlueprintFactory`.

#### 4.2.2 Bytecode

```rust
#[repr(u8)]
pub enum Op {
    PushConst(f32),
    PushFeature(FeatureRef),    // forces LazyValue::as_f32 transitively
    Add, Sub, Mul, Div, Neg, Pow,
    Min, Max, Clamp,            // Clamp pops [val, lo, hi]
    If,                          // pops [cond, then, else]
    Sigmoid, Relu, Tanh, Log, Exp, Sqrt, Abs,
}
```

VM operates on a small operand stack (`SmallVec<[f32; 16]>` — typical programs stay under 16 deep). Max stack depth statically checked by the type checker.

```rust
pub struct ExprExecutor {
    code: Arc<[Op]>,
    stack_cap: u8,                          // computed at compile time
    inputs: SmallVec<[FeatureRef; 8]>,
    out: u8,                                 // output index
}

impl FeatureExecutor for ExprExecutor {
    fn bind(&mut self, _inputs: &[LazyValue], _outputs: &mut [OutputSlot]) { … }
    fn execute(&mut self, doc: DocHandle, ctx: &mut ScoreCtx) {
        let mut stack: SmallVec<[f32; 16]> = SmallVec::with_capacity(self.stack_cap as usize);
        for op in self.code.iter() {
            match op {
                Op::PushConst(c) => stack.push(*c),
                Op::PushFeature(r) => { let v = self.lv(*r).as_f32(doc, ctx); stack.push(v); }
                Op::Add => { let b=stack.pop().unwrap(); let a=stack.pop().unwrap(); stack.push(a+b); }
                // … etc
            }
        }
        self.write_output(stack.pop().unwrap());
    }
}
```

#### 4.2.3 Type checker

Visits the AST bottom-up:
- Numeric literals → `F32`.
- Feature references → output kind of the resolved blueprint (must be `F32` for scalar expressions; tensor-producing features rejected in v1).
- Arithmetic ops → `F32`.
- Max stack depth tracked; if > 32 reject as malformed.
- Function arity validated.
- Side-effect-free guarantee enforced (no loops, no recursion possible by grammar).

#### 4.2.4 JIT (R-8 optional)

Same `FeatureExecutor` trait. Cranelift backend in `proximadb-rank-expr::jit` (feature-gated `rank-jit`). Compile threshold: expressions with > 6 ops *and* invoked > 1000 times warm. Otherwise stay interpreted. (Vespa's heuristic is similar — only scalar expressions JIT, tensors stay interpreted.)

### 4.3 Crate `proximadb-rank-onnx`

#### 4.3.1 `scorer_session.rs`

```rust
pub struct ScorerSession {
    pub model_id:  String,
    pub version:   String,
    pub session:   ort::Session,                 // mmap'd, shared
    pub tokenizer: Option<Arc<tokenizers::Tokenizer>>,
    pub wire:      WirePlan,                     // input/output names + dtypes
    pub input_dtype: DType,
    pub output_dim:  usize,
    pub last_used: std::sync::atomic::AtomicI64, // ms-since-epoch, drives LRU
    refcount:      std::sync::atomic::AtomicUsize,
}
```

`ScorerSession` is the **shared scoring primitive extracted from `proximadb-embedding`**. The embedding crate keeps its high-level `Models::embed_batch_at_precision` API; both crates depend on `proximadb-rank-onnx::scorer_session`.

#### 4.3.2 `model_cache.rs`

```rust
pub struct OnnxModelCache {
    sessions: dashmap::DashMap<ModelKey, Arc<ScorerSession>>,
    registry: Arc<dyn ModelRegistry>,
    memory_budget_bytes: usize,
    eviction_policy: EvictionPolicy,
}

#[derive(Eq, PartialEq, Hash, Clone)]
pub struct ModelKey { pub model_id: String, pub version: String }

pub enum EvictionPolicy {
    LruByMemory { budget_bytes: usize },
    LruByCount  { max_entries:  usize },
    Tenanted    { per_tenant_budget_bytes: usize }, // RLS-aware
}

impl OnnxModelCache {
    pub async fn acquire(&self, key: ModelKey) -> RankResult<ScorerToken> { … }
    pub fn evict_if_over_budget(&self) -> usize { … }
}

/// Refcounted handle. Drop decrements; model not actually unloaded until in-flight tokens drop.
/// Cf. Vespa OnnxModelCache::Token.
pub struct ScorerToken(Arc<ScorerSession>);
```

`acquire` is async because cold loads may need to download the artifact from object storage.

#### 4.3.3 Batch protocol

Cross-encoders must batch. The `OnnxScorerExecutor`'s per-doc `execute()` does *not* call the model — it appends inputs to `ctx.batch`. The pipeline calls `end_of_phase()` which flushes:

```rust
pub struct BatchScratch {
    pub pending: Vec<(DocHandle, Vec<i64>)>,    // (doc, token_ids)
    pub max_batch: usize,
    pub results: HashMap<DocHandle, f32>,
}

impl FeatureExecutor for OnnxScorerExecutor {
    fn execute(&mut self, doc: DocHandle, ctx: &mut ScoreCtx) {
        let ids = self.tokenize_inputs_for(doc, ctx);
        ctx.batch.pending.push((doc, ids));
        if ctx.batch.pending.len() >= ctx.batch.max_batch {
            self.flush(ctx);
        }
    }
    fn end_of_phase(&mut self, ctx: &mut ScoreCtx) -> RankResult<()> {
        if !ctx.batch.pending.is_empty() { self.flush(ctx); }
        Ok(())
    }
}
```

After `end_of_phase`, the pipeline reads `ctx.batch.results[doc]` to populate per-doc scores in the second-phase output slots.

### 4.4 Crate `proximadb-rank-features`

Each built-in feature is a small `Blueprint` impl. File-per-feature for grep-ability.

| Feature | Blueprint | Inputs | Output | Per-doc cost |
|---|---|---|---|---|
| `attribute(field)` | `AttributeBlueprint` | column reader | f32 | O(1) |
| `closeness(field)` | `ClosenessBlueprint` | candidate's cached retrieval distance | f32 | O(1) |
| `cosine(field, query_vec)` | `CosineBlueprint` | tensor field, query vec from `QueryContext` | f32 | O(d) |
| `bm25(field)` | `Bm25Blueprint` | inverted-index stats, doc-len, df | f32 | O(unique_terms) |
| `freshness(field, half_life_s)` | `FreshnessBlueprint` | timestamp attribute | f32 | O(1) |
| `decay(field, half_life)` | `DecayBlueprint` | numeric attribute | f32 | O(1) |
| `model(model_id, [inputs])` | `ModelBlueprint` (delegates to `OnnxScorerExecutor`) | tokenizer + query/doc tensors | f32 | batched O(model) |
| `rankingExpression(expr)` | `ExprBlueprint` | sub-features referenced by expr | f32 | O(\|expr\|) |
| `cross_modal_score(strategy)` | `CrossModalBlueprint` (delegates to existing `CrossModalReranker`) | full hit set | f32 | O(k log k) |

All registered into a shared `BlueprintFactory` at server startup via a `register_builtins(factory: &BlueprintFactory)` function exposed by the crate.

### 4.5 Crate `proximadb-rank-profile`

#### 4.5.1 DSL — TOML form (also expressible inline)

```toml
# A rank profile attached to a collection. Persisted as an xCatalog resource.
[rank_profile.semantic_plus_ce]
inherits = "default"
description = "Hybrid retrieval + cross-encoder rerank"

[rank_profile.semantic_plus_ce.first_phase]
expression = "closeness(embedding) * 0.6 + bm25(title) * 0.4"
heap_size  = 1000

[rank_profile.semantic_plus_ce.second_phase]
expression   = "model('rerank-v3', query, summary)"
rerank_count = 100
batch_size   = 32

[rank_profile.semantic_plus_ce.global_phase]
strategy     = "cross_modal"               # routes to CrossModalReranker
rerank_count = 50

[rank_profile.semantic_plus_ce.match_features]
features = ["bm25(title)", "closeness(embedding)"]

[rank_profile.semantic_plus_ce.summary_features]
features = ["model('rerank-v3', ...)"]

[rank_profile.semantic_plus_ce.budget]
first_max_us  = 5000      # 5ms
second_max_us = 50000     # 50ms
global_max_us = 100000    # 100ms

[[rank_profile.semantic_plus_ce.function]]
name = "personalized"
args = ["user_id"]
expression = "attribute(user_affinity) * sigmoid(closeness(embedding))"

[[rank_profile.semantic_plus_ce.constant]]
name = "w_bm25"
value = 0.4
```

#### 4.5.2 Schema struct (validated, stored in xCatalog)

```rust
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct RankProfileSpec {
    pub name: String,
    pub inherits: Option<String>,
    pub description: Option<String>,
    pub first_phase: Option<PhaseSpec>,
    pub second_phase: Option<PhaseSpec>,
    pub global_phase: Option<GlobalPhaseSpec>,
    pub match_features: Vec<String>,
    pub summary_features: Vec<String>,
    pub budget: PhaseBudgetSpec,
    pub functions: Vec<FunctionSpec>,
    pub constants: Vec<ConstantSpec>,
    pub created_at_ms: i64,
    pub version: u32,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct PhaseSpec {
    pub expression: String,
    pub heap_size: Option<u32>,            // first phase
    pub rerank_count: Option<u32>,         // second phase
    pub batch_size: Option<u32>,
}

pub struct GlobalPhaseSpec {
    pub strategy: String,                  // "cross_modal" | "expression" | "llm_listwise"
    pub rerank_count: Option<u32>,
    pub config: serde_json::Value,         // strategy-specific
}
```

#### 4.5.3 Validation

The profile validator:
1. Parses each `expression` via `proximadb-rank-expr`.
2. Resolves every feature reference against `BlueprintFactory` (rejects unknowns).
3. Builds the DAG, checks for cycles, checks depth ≤ 256.
4. Type-checks expressions.
5. Validates phase budgets are non-zero if specified.
6. Validates `rerank_count ≤ heap_size`.
7. Resolves `inherits` (single inheritance, max chain depth 8).

Validation runs at profile-create time and again at server-startup for every collection's attached profile. Bad profiles are quarantined with a catalog flag `validation_failed=true` and queries fall back to no-profile behavior.

#### 4.5.4 xCatalog binding

```rust
pub trait RankProfileRepository: Send + Sync {
    async fn create(&self, spec: RankProfileSpec) -> RankResult<u32>;     // returns version
    async fn update(&self, spec: RankProfileSpec) -> RankResult<u32>;
    async fn delete(&self, name: &str) -> RankResult<()>;
    async fn get(&self, name: &str) -> RankResult<Option<RankProfileSpec>>;
    async fn list(&self) -> RankResult<Vec<RankProfileSpec>>;
    /// Stream change notifications (new versions, deletions).
    fn watch(&self) -> tokio::sync::broadcast::Receiver<ProfileEvent>;
}
```

Implementations:
- `XCatalogRankProfileRepository` — backed by the existing catalog metadata store (RocksDB / etcd in distributed mode).
- `InMemoryRankProfileRepository` — for embedded / test.

#### 4.5.5 Hot-reload (RCU)

Each query hands out `Arc<CompiledRankProfile>`. When the catalog watcher fires, a new compiled profile is materialized and atomically swapped into the per-collection registry. In-flight queries continue with their captured Arc; the old version is dropped only after the last in-flight reference goes away. No locks on the hot path.

### 4.6 Module `src/core/search/results.rs` — ScoreVector promotion

#### 4.6.1 Current shape (today)

```rust
pub struct OptimizedSearchRecord {
    pub id: String,
    pub score: f32,
    pub similarity: Option<f32>,
    // … modality fields …
}
```

#### 4.6.2 New shape (R-0)

```rust
pub struct OptimizedSearchRecord {
    pub id: String,
    pub score: f32,                          // KEEP — backwards compat; mirrors score_vector.primary
    pub similarity: Option<f32>,             // KEEP
    pub score_vector: Option<ScoreVector>,   // NEW — None when no profile attached
    // … modality fields …
}

/// Multi-component score, populated when a rank profile is attached.
/// `primary` always equals the value driving sort order.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScoreVector {
    pub primary: f32,
    pub phase: PhaseId,                     // which phase produced primary
    pub components: Arc<[ScoreComponent]>,  // owned slice; empty when no match_features
}

/// Moved from crates/query/proximadb-query/src/reranking.rs:481-492.
/// Crate `proximadb-query` re-exports for backwards compat.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ScoreComponent {
    pub name: String,
    pub value: f64,
    pub weight: f64,
    pub contribution: f64,
}
```

**Why `Option<ScoreVector>` and `Arc<[ScoreComponent]>`:**
- `Option` keeps the no-profile case zero-cost (NFR-9). Discriminant-only enum is one word; `None` codegen is identical to today.
- `Arc<[T]>` over `Vec<T>` because score components are shared across copies of the same record during merge/sort phases — avoid the per-clone allocation.

#### 4.6.3 Backwards compatibility migration

1. **Canonical types live in `proximadb-kernel::score_types`.** Both the root crate (`src/core/search/results.rs`) and the workspace crate `proximadb-query` depend on `proximadb-kernel`, so the foundation crate is the only place that breaks the otherwise-acyclic dependency direction. The root re-exports `pub use proximadb_kernel::{PhaseId, ScoreComponent, ScoreVector};` and `proximadb-query::reranking` does `pub use proximadb_kernel::ScoreComponent;` so existing call sites compile unchanged.
2. **`OptimizedSearchRecord::score` stays.** The pipeline writes both `score` and `score_vector.primary` to the same value; readers that only need scalar score continue to work. The `with_score_vector` builder method enforces this mirror.
3. **Serde**: `score_vector` is `#[serde(default, skip_serializing_if = "Option::is_none")]`; `ScoreVector::components` carries the same skip-when-empty discipline (custom `arc_components_serde` adapter for `Arc<[T]>` since serde lacks a derive for owned slices). On-wire payloads stay identical when unused (NFR-9).
4. **Protocol**: gRPC adds an optional `score_vector` field with new tag (no renumbering); REST adds `score_vector` to the JSON response. Both omitted when None.

### 4.7 Model registry — `src/services/model_registry/`

```rust
pub trait ModelRegistry: Send + Sync {
    async fn register(&self, descriptor: ModelDescriptor) -> Result<ModelVersion>;
    async fn get(&self, model_id: &str, version: Option<&str>) -> Result<ResolvedModel>;
    async fn list(&self, tenant: Option<&str>) -> Result<Vec<ModelDescriptor>>;
    async fn delete(&self, model_id: &str, version: &str) -> Result<()>;
}

#[derive(Clone, Debug)]
pub struct ModelDescriptor {
    pub model_id: String,
    pub version:  String,
    pub tenant:   Option<String>,            // RLS scope
    pub uri:      String,                    // s3://… or file://…
    pub sha256:   [u8; 32],
    pub size_bytes: u64,
    pub dtype:    DType,                     // FP32 / FP16 / INT8
    pub input_spec:  Vec<TensorIoSpec>,
    pub output_spec: Vec<TensorIoSpec>,
    pub framework: ModelFramework,           // Onnx for v1; future: Candle, Burn
}

pub struct ResolvedModel {
    pub descriptor: ModelDescriptor,
    pub local_path: std::path::PathBuf,      // post-download
}
```

- WAL: descriptor is logged via the catalog WAL; the model blob is content-addressed in object storage.
- Lazy download: `acquire` triggers download on first reference, with concurrent-fetch dedup (single in-flight per `(model_id, version)`).
- LRU eviction by `(last_used_at, refcount==0)`; never evict in-use sessions.

### 4.8 RL planner extension

```rust
// src/query/rl_planner/mod.rs

/// New action dimension joined to the existing retrieval-plan action.
#[derive(Clone, Debug, Hash, Eq, PartialEq)]
pub struct RankPhaseChoice {
    pub skip_second:    bool,
    pub skip_global:    bool,
    pub second_phase_k: u16,
    pub batch_size:     u16,
}

/// Joint action — extends the existing PlannerAction.
pub struct PlannerAction {
    pub retrieval_plan: RetrievalPlanChoice,  // existing
    pub rank_choice:    RankPhaseChoice,      // new
}

/// Reward augmented with a quality estimate when match-features are emitted
/// and a labeled set is available (offline calibration).
pub struct PlannerReward {
    pub latency_ms: f32,
    pub recall:     f32,
    pub throughput: f32,
    pub memory:     f32,
    pub quality:    Option<f32>,    // new — derived from match-features + held-out labels
}
```

This satisfies CLAUDE.md's no-parallel-implementation mandate — we extend, not duplicate.

### 4.9 Multi-protocol surface

#### 4.9.1 REST

```
POST /v1/collections/{c}/search
{
  "query_vector": [...],
  "k": 50,
  "rank_profile": "semantic_plus_ce",
  "rank_overrides": {
    "second_phase": { "rerank_count": 200 }
  }
}

Response:
{
  "hits": [
    {
      "id": "doc_42",
      "score": 0.876,
      "score_vector": {
        "primary": 0.876,
        "phase": 2,
        "components": [
          { "name": "bm25(title)",  "value": 12.4, "weight": 0.4, "contribution": 4.96 },
          { "name": "closeness(embedding)", "value": 0.91, "weight": 0.6, "contribution": 0.546 },
          { "name": "model(rerank-v3)", "value": 0.87, "weight": 1.0, "contribution": 0.87 }
        ]
      },
      "match_features": { "bm25(title)": 12.4, "closeness(embedding)": 0.91 }
    }
  ],
  "phase_truncated": false,
  "rank_profile": "semantic_plus_ce",
  "rank_profile_version": 7
}
```

#### 4.9.2 gRPC

New fields added to `proximadb.v1.SearchRequest` / `SearchResponse`:

```proto
message SearchRequest {
  // … existing fields …
  optional string rank_profile = 30;
  optional RankOverrides rank_overrides = 31;
}

message RankOverrides {
  optional PhaseOverride second_phase = 1;
  optional PhaseOverride global_phase = 2;
}

message PhaseOverride {
  optional uint32 rerank_count = 1;
  optional uint32 batch_size = 2;
}

message ScoredHit {
  // … existing fields …
  optional ScoreVector score_vector = 20;
  map<string, double> match_features = 21;
  map<string, double> summary_features = 22;
}

message ScoreVector {
  float primary = 1;
  uint32 phase = 2;
  repeated ScoreComponent components = 3;
}

message ScoreComponent {
  string name = 1;
  double value = 2;
  double weight = 3;
  double contribution = 4;
}
```

#### 4.9.3 Arrow Flight

New action `rank_features_export`:

```
DescriptorPath: ["rank_features_export", collection_id, profile_id]
Body: { query_vector?: bytes, k: uint32, since_ts?: int64 }

Returns RecordBatch with schema:
  doc_id: utf8
  query_id: utf8
  feature_name: utf8
  value: float64
  phase: uint8
  weight: float64
  ts_ms: int64
```

Used by offline training pipelines to extract feature/label batches.

#### 4.9.4 pgwire

New SRF (set-returning function):

```sql
SELECT id, score, components
FROM RERANK(
  'my_collection',
  '[0.1,0.2,...]'::vector,
  k => 50,
  profile => 'semantic_plus_ce'
) AS r(id text, score real, components jsonb);
```

Registered via the existing pgwire function-registry binding.

### 4.10 Observability schema

| Metric | Type | Labels | Notes |
|---|---|---|---|
| `rank_phase_latency_us` | Histogram | `profile, phase` | per-phase wall-clock |
| `rank_feature_latency_us` | Histogram | `profile, phase, feature` | per-feature wall-clock |
| `rank_feature_contribution` | Histogram | `profile, feature` | distribution of contribution values |
| `rank_phase_truncated_total` | Counter | `profile, phase, reason` | reason ∈ {budget, error} |
| `rank_model_cache_hit_ratio` | Gauge | `model_id` | rolling ratio |
| `rank_model_cache_size_bytes` | Gauge | — | total memory held |
| `rank_model_evictions_total` | Counter | `model_id, reason` | |
| `rank_model_inflight_loads` | Gauge | — | concurrent cold loads |
| `rank_profile_reload_total` | Counter | `profile, outcome` | outcome ∈ {ok, error} |

All registered through the existing `src/observability/` Prometheus glue (no new infra).

### 4.11 EXPLAIN integration (ADR-004)

```
Search [collection=docs, k=50]
└─ RankPipeline [profile=semantic_plus_ce, version=7]
   ├─ Phase first [heap=1000, budget=5ms, actual=3.2ms]
   │  └─ expression: closeness(embedding)*0.6 + bm25(title)*0.4
   │     ├─ closeness(embedding)
   │     └─ bm25(title)
   ├─ Phase second [k=100, batch=32, budget=50ms, actual=27.4ms, truncated=false]
   │  └─ model('rerank-v3', query, summary)
   └─ Phase global [k=50, budget=100ms, actual=12ms]
      └─ cross_modal_score(mmr_lambda=0.7)
```

Implemented by `RankPipeline::explain() -> ExplainNode`.

---

## 5. Concurrency & memory model

### 5.1 Threading

- Per-query: one `Arc<RankPipeline>` (immutable once compiled) is shared across all shard worker threads.
- Per-thread: a *clone* of each phase's `RankProgram` (built from the pipeline's `RankProgramTemplate`). Cloning is cheap because executors are `Box<dyn FeatureExecutor>` constructed fresh per query (state lives in the executor instance, not in the template).
- Per-thread context (`ScoreCtx`) is `!Send` to encourage placement on the worker that owns the segment scan.
- Async only at phase boundaries: first / second phases use `spawn_blocking` onto the shared search runtime (matching the existing `LocalShard::do_search` pattern). Global phase uses tokio because it may call out to LLM APIs.

### 5.2 Memory budgets

| Source | Budget enforcement |
|---|---|
| Per-query arena | high-water gauge; warn if exceeds `RANK_ARENA_WARN_BYTES` (default 64 MiB) |
| Model cache | `EvictionPolicy::LruByMemory { budget_bytes }`; configurable per node |
| Per-tenant model cache | `EvictionPolicy::Tenanted { per_tenant_budget_bytes }` |
| Feature DAG depth | hard cap 256 (same as Vespa) |
| Expression op count | hard cap 1024 per expression |

### 5.3 RCU profile reload

```rust
struct ProfileRegistry {
    by_name: dashmap::DashMap<String, arc_swap::ArcSwap<CompiledRankProfile>>,
}
// Reader:
let profile = self.by_name.get(name).map(|s| s.load_full());  // O(1), lock-free
// Writer:
let entry = self.by_name.entry(name.to_string()).or_default();
entry.store(Arc::new(new_compiled));   // atomic swap; old still readable by in-flight queries
```

---

## 6. Test strategy

All tests inline (`#[cfg(test)] mod tests`) per CLAUDE.md unless they need cross-crate fixtures, in which case `tests/rank/`.

### 6.1 R-0 — ScoreVector promotion

| Test | Location | What it asserts |
|---|---|---|
| `score_vector_default_is_none` | `src/core/search/results.rs` | `OptimizedSearchRecord::default().score_vector == None` |
| `score_vector_serde_roundtrip` | same | json round-trips preserve components |
| `score_vector_none_omitted_from_json` | same | `serde_json::to_string` does not contain `score_vector` key when None |
| `score_component_promoted_path_compiles` | `crates/query/proximadb-query/src/reranking.rs` | existing `reranking.rs` callers compile unchanged (re-export check) |
| `optimized_record_default_eq_today` | same | structural equality with a snapshot of today's default to guard against accidental field reorder |
| `score_field_mirrors_primary` | same | when `score_vector = Some(v)`, `score == v.primary` |

### 6.2 R-1 — rank-core skeleton

| Test | Asserts |
|---|---|
| `blueprint_factory_register_and_lookup` | factory round-trips |
| `feature_ref_size_is_3_bytes` | layout guard (1 u16 + 1 u8 packed → 4 bytes with padding; assert ≤ 4) |
| `lazy_value_force_memoizes` | second `as_f32` for same doc does not re-run executor |
| `lazy_value_force_resets_per_doc` | new doc resets the memoization bitmap |
| `arena_reset_is_O1` | bench-gated (1ms wall-clock budget for 10k resets) |
| `phase_budget_exceeded_returns_partial` | pipeline with 1us budget yields `PhaseBudgetExceeded` |
| `score_ctx_deadline_propagates` | per-feature execute sees the deadline |

### 6.3 R-2 — built-in features

| Test | Asserts |
|---|---|
| `attribute_blueprint_reads_column` | with a fake `AttributeAccess`, output matches column value |
| `closeness_blueprint_reads_candidate_distance` | retrieval distance flows through |
| `bm25_blueprint_matches_reference_impl` | output within 1e-5 of a hand-computed BM25 |
| `freshness_blueprint_decays_correctly` | half-life property |
| `decay_blueprint_decays_correctly` | half-life property |

### 6.4 R-3 — expression VM

| Test | Asserts |
|---|---|
| `parse_simple_expression` | `bm25(title) * 0.4 + closeness(emb) * 0.6` parses |
| `parse_rejects_unknown_function` | `frobnicate(x)` rejected |
| `parse_rejects_too_deep` | nested > 256 rejected |
| `typecheck_rejects_tensor_feature_in_scalar_context` | tensor-producing feature in arithmetic rejected |
| `bytecode_compile_arity_check` | wrong arity to `if` rejected |
| `vm_evaluates_simple_expression` | direct numeric eval matches f32 reference |
| `vm_evaluates_with_feature_refs` | end-to-end with fake DAG |
| `vm_no_alloc_per_doc` | allocator-counter test (custom global allocator hook) shows zero allocs in steady state |

### 6.5 R-4 — rank profile DSL + xCatalog

| Test | Asserts |
|---|---|
| `parse_full_profile_toml` | round-trips |
| `inheritance_resolves_single_chain` | child overrides parent |
| `inheritance_rejects_cycle` | A→B→A rejected |
| `validation_rejects_unknown_feature` | dangling reference rejected |
| `validation_rejects_rerank_count_gt_heap_size` | budget invariant |
| `xcatalog_persist_and_fetch` | xcatalog round-trips |
| `hot_reload_atomic_swap` | concurrent queries see consistent version |
| `rest_request_with_profile_returns_score_vector` | end-to-end REST integration |

### 6.6 R-5 — ONNX scorer + model registry

| Test | Asserts |
|---|---|
| `model_descriptor_round_trips_through_xcatalog` | persistence |
| `model_acquire_dedups_concurrent_loads` | only one cold load for N concurrent acquires |
| `scorer_token_keeps_session_alive` | drop count drives eviction eligibility |
| `lru_eviction_respects_inuse_refcount` | session with refcount>0 never evicted |
| `onnx_scorer_batches_correctly` | k=100 with batch_size=32 yields exactly ceil(100/32) inference calls |
| `cross_encoder_e2e_with_tiny_test_model` | bundle a 10MB test ONNX in `tests/fixtures/` |
| `model_load_failure_isolates_to_profile` | bad model marks profile validation_failed; collection still queryable without rank |

### 6.7 R-6 — global phase + fusion composition

| Test | Asserts |
|---|---|
| `cross_modal_reranker_adapter_compiles` | existing reranker exposed as `GlobalScorer` |
| `hybrid_then_rerank_pipeline` | RRF → first → second → global produces stable order |
| `rerank_after_fusion_preserves_explainability` | components from all three phases present in `ScoreVector` |
| `weaviate_gap_test` | regression: pipeline with both hybrid and rerank does NOT silently drop rerank (named after the Weaviate explorer_hybrid.go:400 limitation) |

### 6.8 R-7 — RL planner + observability + Arrow Flight

| Test | Asserts |
|---|---|
| `rl_planner_emits_joint_action` | new action shape returned |
| `rl_planner_reward_includes_quality_when_available` | with held-out labels, quality > 0 |
| `prometheus_metrics_emitted_per_feature` | scrape includes all expected labels |
| `flight_export_returns_feature_batch` | Arrow RecordBatch schema matches spec |

### 6.9 Bench gates

| Bench | Target |
|---|---|
| `bench_first_phase_per_doc_single_feature` | ≤ 50 ns / doc |
| `bench_first_phase_per_doc_5_features` | ≤ 250 ns / doc |
| `bench_second_phase_ce_p95` | ≤ 30 ms |
| `bench_arena_reset_p99` | ≤ 1 µs |
| `bench_no_profile_zero_cost` | within 2% of baseline `cargo test` benchmark suite |

Bench gates run in `cargo bench` and a regression fails CI.

---

## 7. Phased rollout (TDD-driven)

| Phase | Deliverable | Dependencies | Est. duration | Acceptance |
|---|---|---|---|---|
| **R-0** | Promote `ScoreComponent`; add `ScoreVector` (`Option<…>`) to `OptimizedSearchRecord`; thread through retrieval | none | 1 sprint | R-0 tests pass; no regression in existing search tests; baseline bench unchanged |
| **R-1** | `proximadb-rank-core` crate skeleton: `Blueprint`, `BlueprintFactory`, `FeatureExecutor`, `LazyValue`, `RankProgram`, `RankPipeline`, `PhaseBudget`, `ScoreCtx`, `FeatureArena` | R-0 | 1 sprint | R-1 tests pass; benches green |
| **R-2** | Built-in features: `attribute`, `closeness`, `bm25`, `freshness`, `decay`. `CandidateStream` wired from retrieval output | R-1; existing hybrid module | 1 sprint | R-2 tests pass; end-to-end first-phase ranking working |
| **R-3** | `proximadb-rank-expr`: PEG parser, AST, type checker, bytecode, interpreter VM, `rankingExpression(…)` feature | R-2 | 1 sprint | R-3 tests pass; alloc-free hot path verified |
| **R-4** | `proximadb-rank-profile`: TOML DSL, schema, validator, inheritance, xCatalog binding, hot-reload via RCU, REST surface | R-3 | 1 sprint | R-4 tests pass; profile CRUD via REST works |
| **R-5** | `proximadb-rank-onnx`: extract `ScorerSession` from embedding crate, build `OnnxModelCache`, LRU eviction, batching protocol. `model(…)` feature. `services/model_registry` | R-4 | 2 sprints | R-5 tests pass; cross-encoder rerank working end-to-end |
| **R-6** | `CrossModalReranker` adapted to `GlobalScorer`. Hybrid coordinator emits `CandidateStream`. Pipeline composes fusion → phases (Weaviate gap closed) | R-5 | 1 sprint | R-6 tests pass; hybrid+rerank works |
| **R-7** | RL planner extension; gRPC + Arrow Flight + pgwire surface; observability emission; EXPLAIN integration | R-6 | 1 sprint | R-7 tests pass; all four protocols return `ScoreVector` |
| **R-8** (opt) | Cranelift JIT for hot expressions behind `rank-jit` feature flag | R-7 | 1 sprint | bench gate showing > 2x speedup vs interpreter on canonical workload |
| **R-9** (opt) | Tensor expression DSL (named dimensions, à la Vespa) | R-8 | 2+ sprints | scope deferred until user pull |

Each phase ends with a green `cargo fmt && cargo clippy -- -D warnings && cargo test && cargo bench` (the bench step gated on the phases that establish bench gates).

---

## 8. Risks

| Risk | Severity | Mitigation |
|---|---|---|
| Borrow-checker friction in LazyValue forcing | M | Documented `unsafe` block (single-threaded per worker); falls back to `RefCell` if it turns out to be too brittle, with a measured 3ns/access cost |
| Cross-encoder OOM under load | H | Tenanted LRU + per-node memory budget; refuse-to-load when budget exceeded; degrade gracefully to skip-second-phase |
| Profile DSL becomes a Turing tar pit | M | Grammar forbids loops/recursion; max op count; max DAG depth; static analysis bounds per-doc cost |
| Duplication with existing `CrossModalReranker` | H (CLAUDE.md mandate) | **Adopted as a `GlobalScorer`**, not duplicated. Existing direct call sites migrate one-by-one to the pipeline |
| Hot-reload breaks in-flight queries | M | RCU pattern (`ArcSwap`); in-flight queries hold their own `Arc<CompiledRankProfile>` snapshot |
| Cranelift adds build dep weight | L | Behind `--feature rank-jit`; default off |
| Multi-protocol divergence | M | Single `RankRequest` proto, all four surfaces lower to it |
| RL planner regresses retrieval quality when learning new actions | M | Action space gated by feature flag `rl_rank_actions`; rollout begins with `epsilon=0` (always exploit known-good action) |
| `ScoreVector` field rename breaks proto wire compat | H | New field with new tag; existing tags untouched; serde `skip_serializing_if = "Option::is_none"` |
| Model artifact tampering | M | sha256 verification in registry; bad checksum → load failure |
| Tenant isolation leakage via shared model cache | H | `Tenanted` eviction policy; key includes tenant scope; RLS check on registry lookup |

---

## 9. Open questions

1. Should rank profiles be **per-collection** or **per-collection-per-modality**? The Vespa model is per-document-type. ProximaDB has multiple modalities in one collection — likely per-collection with optional modality-specific override.
2. Should the LLM listwise reranker be its own crate or live in `rank-features::cross_modal`? Leaning own crate (`proximadb-rank-llm`) once R-6 lands.
3. Cranelift vs LLVM for JIT (R-8)? Cranelift is pure-Rust, smaller dep, less mature for fp math. LLVM is what Vespa uses but adds toolchain dep. Default Cranelift.
4. Should match-features be opt-in per-query or per-profile? Per-profile keeps payload size predictable; per-query is more flexible. Default per-profile, with per-query override.
5. How do match-features integrate with the existing `SearchDebugInfo` field? Probably fold debug info into `score_vector.components` for unified explainability.

---

## 10. References

### ProximaDB code (must reuse)
- `src/core/search/results.rs:204-294` — `OptimizedSearchRecord`
- `src/core/search/hybrid/{mod.rs,coordinator.rs,reranker.rs,bm25_wrapper.rs}` — hybrid fusion
- `crates/query/proximadb-query/src/reranking.rs:481-505` — `ScoreComponent`, `RerankedResult`, `CrossModalReranker`
- `crates/query/proximadb-query-fusion/src/lib.rs` — `FusionStrategy`
- `crates/modalities/proximadb-embedding/src/models/mod.rs:48-120` — `ort` session management, BGE routes
- `crates/modalities/proximadb-embedding/src/tokenizer.rs` — `SharedTokenizer`
- `src/query/rl_planner/mod.rs` — Thompson Sampling planner
- `src/query/unified/executor.rs` — multi-model executor
- `src/storage/engines/factory.rs` — factory pattern reference
- `src/services/` — service crate placement convention

### CLAUDE.md mandates
- Reuse-First Architecture Rules
- Canonical types: `ProximaRecord`, `ProximaType`, `ProximaValue`
- xCatalog as durable authority
- Workspace map: Foundation → Contracts → Modality Runtime → Cross-Model Query Runtime → Platform Runtime → Apps/Bindings
- TDD with pre-commit hooks (`make install-tdd-hooks`)
- Inline `#[cfg(test)]` tests preferred over `tests/` files

### ADRs / design docs
- `docs/12-design/adr/ADR-004-unified-explain-contract.adoc` — EXPLAIN integration
- `docs/12-design/RELATIONAL_DOCUMENT_GRAPH_CONVERGENCE_2026_05_14.adoc` — stacked durability context
- `roadmap/MULTIMODAL_OVERHAUL_SPEC_2026_05_08.adoc` — canonical records spec

### External — Vespa (engineering reference)
- `searchlib/src/vespa/searchlib/fef/{blueprint,featureexecutor,featurenameparser}.{h,cpp}` — Blueprint/Executor pattern
- `searchcore/src/vespa/searchcore/proton/matching/{match_master,match_thread,match_tools}.{h,cpp}` — multi-phase orchestration
- `eval/src/vespa/eval/{eval,instruction,onnx}/` — tensor framework + ONNX integration
- `config-model/src/main/java/com/yahoo/schema/RankProfile.java` — rank profile schema
- `searchlib/src/vespa/searchlib/queryeval/nearest_neighbor_blueprint.cpp` — NN as Blueprint

### External — Weaviate (gap reference)
- `entities/modulecapabilities/module.go` — module interface
- `modules/reranker-cohere/{module.go,clients/ranker.go}` — reranker shape
- `usecases/modulecomponents/additional/rank/rank.go` — closure-based extension
- `usecases/traverser/explorer_hybrid.go:~400` — **the gap comment** ("cannot be performed with hybrid search")

---

## 11. Changelog

| Date | Change |
|---|---|
| 2026-05-23 | Initial draft. R-0 selected as first implementation slice. |
| 2026-05-23 | R-0 landed: `PhaseId`/`ScoreComponent`/`ScoreVector` in `proximadb-kernel::score_types` (10 tests); `OptimizedSearchRecord::score_vector` field + `with_score_vector` builder (5 tests); `proximadb-query::reranking` re-exports kernel `ScoreComponent`; incidental fix — added missing `GraphNode` backwards-compat alias in `src/embedded/mod.rs` (promised at line 509 doc comment but never landed; was blocking the lib-test build). |
