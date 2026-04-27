# ProximaDB Recommendations — Phase 5 Agentic Intelligence

**Refined**: 2026-04-25 (manual review + arXiv verification of auto-generated V3 against current codebase)
**Verification**: All 9 cited arXiv papers fetched and abstracts read on 2026-04-25. Several auto-pipeline summaries were inaccurate — see "Real paper findings" notes.
**Status**: Authoritative companion to `docs/10-quality/TECHNICAL_DEBT.adoc` Phase 5 (TD-043..TD-052)

---

## How to read this document

Each recommendation is a **research-paper-driven enhancement** to ProximaDB's vector / graph / document /
multi-modal layers. For every item:

- **Paper** — verified arXiv ID, title, and authors.
- **What the paper actually proposes** — drawn from reading the abstract directly.
- **Where the auto-pipeline went wrong** (when applicable) — flagged because the original V3
  draft mischaracterized several papers in technically meaningful ways.
- **Status in ProximaDB** — `Done` / `Partial` / `Pending`, anchored to file paths.
- **TD entry** — the corresponding row in `docs/10-quality/TECHNICAL_DEBT.adoc`.
- **Concrete next step** — what a future agent should do next, with file anchors.
- **Acceptance criteria** — how to know it's complete.

If you are an agent picking this up: **read the cited paper first**, then this section. The
auto-generated V3 draft of this document conflated different techniques in places — the entries
below have been corrected against the real abstracts, but you should still confirm before
implementing. Don't trust the V3 summaries that have been preserved historically in commit
messages or the older TD-043..TD-050 entries.

---

## 1. Vector & Document Layer

### 1.1 Semantic Disentanglement via Document Preprocessing
- **Paper**: arXiv:2604.17677 — Loghmani 2026, *"Semantic Entanglement in Vector-Based Retrieval:
  A Formal Framework and Context-Conditioned Disentanglement Pipeline for Agentic RAG Systems"*.
- **What it actually proposes**: A 4-stage **preprocessing pipeline (SDP)** that restructures
  documents *before* embedding, plus a quantitative **Entanglement Index (EI)** that measures
  cross-topic overlap in embedding space. On a 2K-document healthcare KB, SDP improves Top-K
  precision from ~32% to ~82% and reduces mean EI from 0.71 to 0.14.
- **Where the auto-pipeline went wrong**: V3 said "centroid + residual topic vectors in AXIS".
  The paper does **not** modify the index — it modifies the *chunking and preprocessing* of
  documents. Implementing residual indexing would not reproduce the paper's results.
- **Status**: **Pending**.
- **TD entry**: `TD-043` (HIGH).
- **Where in ProximaDB**:
  - Document chunking & ingestion path: `src/services/operations/` (vector/document operations);
    document store lives in `src/storage/engines/impls/` (look for the document/JSON engine).
  - Embedding hooks: places that produce or accept vectors before insertion — search for callers
    of `extract_vector` or vector-record construction.
  - Existing retrieval-precision benchmarks (none for entanglement) live under `benches/`.
- **Concrete next step**:
  1. Implement an `EntanglementIndex` analyzer in a new module `src/analytics/entanglement/` that
     takes a collection's document set + their embeddings and returns the EI metric. This is
     read-only and can land independently as a measurement tool.
  2. Build the SDP preprocessing pipeline as a *configurable chunker* before embedding — the
     paper's 4 stages are document-structure-aware splitting. Wire as an optional pre-ingest
     transform on the document service.
  3. Validate by reproducing the paper's EI ↓ / precision ↑ relationship on at least one public
     dataset (BEIR multi-topic split or a synthesized multi-topic corpus).
- **Acceptance criteria**:
  - EI can be computed for any collection via `GET /api/v1/collections/{id}/entanglement`.
  - SDP pipeline reduces EI on a multi-topic test corpus by ≥30% vs default chunking.
  - Top-K precision improvement ≥10pp on the same corpus (paper claims 50pp; we target a more
    conservative 10pp for a different domain).

### 1.2 Projection-Based Fusion (B5) — Speed/Diversity Tradeoff
- **Paper**: arXiv:2604.13728 — Prajapati 2026, *"Hybrid Retrieval for COVID-19 Literature:
  Comparing Rank Fusion and Projection Fusion with Diversity Reranking"*.
- **What it actually proposes**: Compares 6 retrieval configurations on TREC-COVID. **RRF wins
  on relevance** (nDCG@10 = 0.828, +6.1% over dense-only, +14.9% over sparse-only).
  **Projection fusion (B5) is offered as a faster alternative with greater result diversity**.
- **Where the auto-pipeline went wrong**: V3 framed Projection as a strict improvement over RRF
  ("33% speed AND higher diversity"), implying RRF should be replaced. The paper's actual
  finding is that **RRF is the relevance-optimal default**, and B5 is a speed/diversity-biased
  alternative — not a strict upgrade.
- **Status**: **Implemented** at `src/core/search/hybrid/fusion.rs:106-253`. Needs
  benchmark + correct positioning in docs.
- **TD entry**: `TD-044` (downgrade from HIGH to MEDIUM — it's a tradeoff option, not a flagship
  improvement).
- **Where in ProximaDB**:
  - `src/core/search/hybrid/fusion.rs:106-253` — `projection_fusion()`.
  - `src/core/search/hybrid/mod.rs:213-228` — `FusionStrategy::Projection { alpha }` variant.
  - Tests: `fusion.rs:2074-2160`.
- **Concrete next step**:
  1. Reproduce the paper's headline: on TREC-COVID-style data, confirm RRF beats B5 on nDCG@10
     and B5 beats RRF on latency + intra-list diversity (ILD@10).
  2. Document position clearly in `docs/concepts/` or a fusion guide:
     **"Default RRF for relevance; choose Projection when you need diversity or sub-millisecond
     fusion latency."**
  3. Wire as selectable mode in REST hybrid endpoint and Python SDK so callers can opt in.
  4. Do **NOT** make Projection the default. (This contradicts V3's framing — the paper's data
     does not support it.)
- **Acceptance criteria**:
  - Benchmark report committed under `docs/benchmarks/` showing the relevance/speed/diversity
    tradeoff on a public IR dataset.
  - SDK exposes `hybrid_mode="projection"` with `alpha` parameter; default remains `"rrf"`.

### 1.3 Unified Retrieval + Compression Embedding
- **Paper**: arXiv:2604.14403 — Killingback, Meshi, Li, Zamani, Karimzadehgan 2026 (Google),
  *"A Unified Model and Document Representation for On-Device Retrieval-Augmented Generation"*.
- **What it actually proposes**: A unified model in which the *same* representation is used for
  both retrieval kNN and as the compressed context fed to the LLM. Achieves performance matching
  traditional RAG using ~1/10 of the context, with no extra storage vs multi-vector retrieval.
- **Status**: **Pending**. Recommend as **TD-051**.
- **Where in ProximaDB**:
  - Compute layer: `src/compute/quantization/unified.rs` (where a learned projection would live).
  - Storage layer: document engine + WAL — currently stores embeddings *and* raw context
    separately.
  - Note: this requires an *embedding model trained for this dual use*. ProximaDB does not train
    embeddings — this is a database-side **integration story**, not a database-internal feature.
- **Concrete next step**:
  1. Add a `DualUseEmbedding` collection metadata flag indicating "embeddings are also the
     compressed context representation" — when set, the DB skips storing raw context bytes and
     reconstructs at query time via a registered decompression callback.
  2. Add a Python SDK example showing how to plug in a compatible model (e.g., the paper's
     model when released) so users can enable the optimization without DB changes.
  3. Do NOT bake a specific compression scheme into the storage engine — the model is the
     contract, not the bytes.
- **Acceptance criteria**:
  - Collection config flag `embedding.dual_use = true` honored end-to-end.
  - Storage footprint with dual_use enabled is ≤55% of the equivalent embedding+raw-text
    collection on a representative corpus.

---

## 2. Graph Layer

### 2.1 Modular Graph RAG (RGL)
- **Paper**: arXiv:2503.19314 — Li, Hu, Jiang, Liu, Hooi, He 2025 (NUS),
  *"RGL: A Graph-Centric, Modular Framework for Efficient Retrieval-Augmented Generation on Graphs"*.
- **What it actually proposes**: A modular library spanning the full pipeline — graph indexing,
  *dynamic node retrieval*, subgraph construction, tokenization, generation. Reports **143×
  speedup over conventional methods** through optimized component implementations. Emphasizes
  *dynamic node filtering* as the main lever for reducing token consumption.
- **Status**: **Pending**.
- **TD entry**: `TD-045` (CRITICAL).
- **Where in ProximaDB**:
  - Graph engines: `src/graph/engines/` (ORION default).
  - Hybrid vector+graph: `src/graph/hybrid/` (QUASAR — feature-gated).
  - Traversal API: `src/graph/service_traversal_api.rs`.
  - Need new module: `src/graph/rag/` (does not exist).
- **Concrete next step**:
  1. Read RGL's GitHub release if available to see their actual component split before designing
     ours — the paper emphasizes "graph database community insights" so the contribution is
     largely *engineering* rather than novel algorithms.
  2. Define two trait surfaces in `src/graph/rag/`:
     - `NodeRetriever`: `query -> Vec<NodeId>` (vector-based, BM25, hybrid).
     - `SubgraphBuilder`: `seeds -> Subgraph` (k-hop, Personalized PageRank, Steiner).
  3. Implement *dynamic filtering* — the paper's headline lever — at the boundary between
     retriever and builder. This is where the 143× claim comes from.
  4. Expose via `POST /api/v1/graph/{id}/rag`.
- **Acceptance criteria**:
  - Composition matrix: ≥2 retrievers × ≥2 builders, all combinations tested.
  - On a 1M-node graph: k=5 retrieval + 2-hop subgraph at limit=100 returns p95 <100ms.
  - Demonstrated token-budget reduction vs naive BFS dump (target: ≥5× reduction).

### 2.2 Tool-Based Navigation (GraphWalk)
- **Paper**: arXiv:2604.01610 — Ghandi, Mahyar, Klaiman 2026,
  *"GraphWalk: Enabling Reasoning in Large Language Models through Tool-Based Graph Navigation"*.
- **What it actually proposes**: Equip LLMs with a *minimal set of graph operations* (move, look,
  remember) so they can traverse arbitrary structures via tool calls. Tested on maze traversal
  and synthetic enterprise KGs. Gains "more pronounced as scale increases" — i.e., the tool
  approach beats subgraph-dump approaches as graphs get larger.
- **Status**: **Partial — primitive exists, not yet exposed as a tool**.
- **TD entry**: `TD-046` (HIGH).
- **Where in ProximaDB**:
  - Primitive: `src/graph/service_traversal_api.rs:355-410` — `graph_walk(graph_id, start, depth, limit)`.
  - Service test: `src/graph/service.rs` `test_graph_walk_tool`.
  - Note: the current implementation does breadth-first expansion in one call. The paper's
    contribution is **iterative single-step navigation** — agent picks one neighbor per call.
- **Concrete next step**:
  1. Add a *single-step* primitive `graph_step(graph_id, current_node, edge_filter) ->
     {neighbors, properties}` alongside the existing breadth-bounded walk. This matches the
     paper's tool interface more directly.
  2. REST endpoint `POST /api/v1/graph/{id}/walk` and `POST /api/v1/graph/{id}/step`.
  3. Proto messages in `proto/proximadb/v1/graph.proto` + gRPC handlers.
  4. MCP / OpenAI function-call schema doc so agents can register the tool.
  5. Add edge-filter predicate support and per-step result projection.
  6. LangChain/CrewAI tool integration in `clients/python/src/proximadb_sdk/integrations/`.
- **Acceptance criteria**:
  - Agent can iteratively traverse a 1M-node graph using ≤8K tokens per step.
  - Both BFS-bounded `walk` and single-step `step` interfaces available.
  - Edge filter and projection covered by inline tests.

---

## 3. Multi-modal & Query Optimization

### 3.1 Evolutionary Query Planning with LLM-Proposed Edits
- **Paper**: arXiv:2602.10387 — Erol, Hao, Bianchi, Greco, Tagliabue, Zou 2026,
  *"Making Databases Faster with LLM Evolutionary Sampling"*.
- **What it actually proposes**: A **DBPlanBench** harness for DataFusion. Expose physical plans
  in a compact serialized representation; **let an LLM propose localized plan edits**;
  evolutionary search refines candidates over iterations. Reports up to **4.78× speedup** and
  shows that optimizations transfer from small to large databases.
- **Where the auto-pipeline went wrong** (and the V2 of TD-047 too): The novelty is **LLM-as-
  mutation-operator**, not generic evolutionary search. The current
  `src/query/unified/evolutionary.rs` skeleton uses random topological-order swaps for mutation
  — that's classical genetic algorithm scheduling, not what the paper does.
- **Status**: **Partial — generic evolutionary scaffold exists; LLM-mutation operator does not**.
- **TD entry**: `TD-047` (re-scoped: MEDIUM, two sub-deliverables).
- **Where in ProximaDB**:
  - Skeleton: `src/query/unified/evolutionary.rs` (untracked — must commit).
  - Wired into: `src/query/unified/optimizer.rs:226-242` (gated, default off).
  - DataFusion integration (the paper's vehicle): see ProximaDB's existing
    `ProximaTableProvider`/`ProximaScanExec` — could reuse the paper's harness directly.
- **Concrete next step**:
  1. **Commit the current skeleton** (`evolutionary.rs` is untracked in git as of 2026-04-25).
  2. Land **measured-time fitness**: cache wall-clock execution time of trial plans, use that as
     the cost function. The current `cost_fn: Fn(&[SelectivityEstimate], &[usize]) -> f64`
     callback in `evolutionary.rs:28-35` supports this with no signature change.
  3. **Separate concern**: the paper's contribution (LLM mutation operator + DBPlanBench harness)
     is a follow-up. Spec it under `docs/_internal/roadmap/specifications/` first; only build
     after Context Lake decisions clarify whether ProximaDB will have an LLM dependency on the
     hot path.
  4. Add regression benchmark `benches/query_optimizer.rs` verifying the evolutionary planner
     never produces a slower plan than greedy on a fixed test set.
- **Acceptance criteria**:
  - `evolutionary.rs` committed.
  - Plan-execution-time cache stores ≥100 historical plans per query shape with bounded memory.
  - Benchmark gate: ≥5% improvement over greedy on 4+ component federated joins; within 5%
    on simple queries.
  - LLM-mutation work tracked as a separate sub-task with its own spec doc.

### 3.2 Agentic View Decomposition (AV-SQL)
- **Paper**: arXiv:2604.07041 — Pham, Pham, Chen, Yin, Nguyen, Nguyen 2026,
  *"AV-SQL: Decomposing Complex Text-to-SQL Queries with Agentic Views"*.
- **What it actually proposes**: A **3-agent pipeline**:
  1. *Rewriter agent* compresses and clarifies the input NL query.
  2. *View generator agent* processes schema chunks to produce per-modality "agentic views".
  3. *Planner / generator / revisor agents* compose the views into the final SQL.
  Reports **70.38% execution accuracy on Spider 2.0**.
- **Status**: **Pending**.
- **TD entry**: `TD-048` (HIGH).
- **Where in ProximaDB**:
  - Existing decomposition starting point: `src/query/unified/decomposition.rs`.
  - SQL frontend: `src/query/federated/`.
  - LLM integration: `[llm]` config section in `config/config.toml`.
- **Concrete next step**:
  1. Mirror the paper's 3-agent split as 3 traits in a new module `src/query/nl/`:
     `Rewriter`, `ViewGenerator`, `ComposerAgent`. Default impls can be rule-based for v1.
  2. LLM-backed impls behind feature flag `llm-nl-query`.
  3. New REST endpoint `POST /api/v1/query/nl` returns the assembled SQL **plus** the
     intermediate views (paper emphasizes traceability — match this).
  4. Spider 2.0 evaluation harness in `benches/` — even hitting 30% would be a meaningful
     baseline; the paper's 70% is with a tuned LLM stack.
- **Acceptance criteria**:
  - End-to-end NL→federated-SQL test for a query like "find authors who cited papers about X"
    (vector + graph + document join).
  - Response includes intermediate views for transparency.
  - Spider 2.0 execution accuracy ≥30% on a sample (lower bar than the paper, scoped to
    ProximaDB's available schema connectors).

### 3.3 Block-Batched Semantic Joins
- **Paper**: arXiv:2510.08489 — Trummer 2025 (Cornell),
  *"Implementing Semantic Join Operators Efficiently"*.
- **What it actually proposes**: **Block nested loops join** for semantic joins — pack batches
  of rows from *both* tables into a single LLM prompt and ask the model to identify all matching
  pairs in one shot. Includes formulas for optimal batch size given context limits, plus an
  adaptive variant. Significant cost reduction vs nested-loop-per-pair.
- **Where the auto-pipeline went wrong**: V3 framed this as "evolutionary sampling, O(n+m)
  calls". The paper's algorithm is **block nested loops with batched prompts** — a well-defined,
  classical join technique adapted for LLMs, not an evolutionary one. Different cost model,
  different complexity analysis, different implementation.
- **Status**: **Partial — cosine-only semantic join exists; block-batched LLM join does not**.
- **TD entry**: `TD-049` (rename to "Block-Batched Semantic Joins"; HIGH).
- **Where in ProximaDB**:
  - Existing impl: `src/query/unified/executor.rs:1619-1707` — `execute_semantic_join()`
    (cosine-similarity-based; no LLM).
  - Join AST: `src/query/unified/ast.rs:148-156` — `JoinType::Semantic { threshold, top_k }`.
- **Concrete next step**:
  1. Rewrite TD-049's title and description (in TECHNICAL_DEBT.adoc and any commit messages
     that referenced "evolutionary semantic joins") to "block-batched semantic joins".
  2. Extend `JoinType::Semantic` with a `mode` enum: `Cosine` (current) | `LlmBlockBatch
     { batch_size_left, batch_size_right, max_calls }`.
  3. Implement the paper's optimal batch-size formula given the configured LLM context window.
  4. Add the adaptive variant (handles cases where output size is hard to predict).
  5. Hard-cap LLM calls per query; expose a Prometheus metric for observability.
- **Acceptance criteria**:
  - On a labeled NL-condition dataset, block-batched mode reduces LLM call count by ≥10×
    vs nested-loop baseline at equal precision@10.
  - LLM call count is bounded and observable.

---

## 4. Agentic AI & Memory

### 4.1 RUBICON / AQL — Auditable Agentic Query Plans
- **Paper**: arXiv:2604.21413 — Wenz, Treutwein, Arenja, Demiralp, Stonebraker 2026,
  *"An Alternate Agentic AI Architecture (It's About the Data)"*.
- **What it actually proposes**: **RUBICON** architecture centered on data management instead of
  reasoning. Defines **AQL (Agentic Query Language)** with `Find`, `From`, `Where` operators
  executed through source-specific wrappers. The thesis: enterprises need **traceability,
  determinism, and trust**, which opaque LLM agent chains can't provide; explicit query plans
  can.
- **Where the auto-pipeline went wrong**: V3 branded this as "Context Lake / DBOS principles",
  evoking durable execution and unified memory. The paper does NOT discuss DBOS or a memory
  store. Its contribution is a **query language and architectural pattern** for *replacing* LLM
  agent chains with auditable plans. This is much closer to a federated query rewriter than to
  agent memory persistence.
- **Status**: **Pending — architectural pattern, not a single feature**.
- **TD entry**: `TD-050` (rename to "RUBICON-Style Auditable Query Layer"; CRITICAL).
- **Where in ProximaDB**:
  - SQL extensions: `src/query/unified/` — already supports `VECTOR_SEARCH()`, `GRAPH_QUERY()`,
    `DOCUMENT_QUERY()`. AQL's Find/From/Where maps onto this surface.
  - Federated execution: `src/query/federated/`.
  - Audit logging: `src/security/` (audit trail infrastructure).
- **Concrete next step**:
  1. **Read the paper in full** before designing — the abstract is suggestive but the operator
     semantics matter. Check whether the authors publish a reference implementation.
  2. Map AQL Find/From/Where onto existing federated query primitives. ProximaDB already has
     most of the substrate (cross-model joins, federated execution); the gap is the **audit
     story**.
  3. Build an "explain plan" surface that produces a structured, auditable trace of every
     federated query step (which engine, which collection, which filters, which costs).
  4. Spec under `docs/_internal/roadmap/specifications/RUBICON_ALIGNMENT.adoc`.
  5. *Drop* the "Context Lake" name everywhere — the paper does not justify that branding.
- **Acceptance criteria**:
  - Spec doc reviewed.
  - `EXPLAIN VERBOSE` for any federated query produces a trace usable for compliance auditing
    (which sources accessed, what filters applied, which model returned what).
  - At least one customer-facing example of "agentic NL query → AQL plan → audit trail".

### 4.2 Proactive Agentic Memory (Speculative — No Citation)
- **Paper**: **No verified citation**. The "PASK" acronym was *not* a real research term — the
  V3 draft introduced it without a source.
- **Status**: **Pending — speculative, no paper backing**.
- **TD entry**: `TD-052` (LOW; downgrade from MEDIUM since it has no concrete research basis).
- **Recommendation**:
  1. Treat as future research direction, not a planned feature.
  2. If pursued: integrate with the existing Thompson Sampling planner in
     `src/query/unified/optimizer.rs` (RL backbone) and AutoML infrastructure `src/automl/`;
     reward signal would be agent task success.
  3. **Wait** until TD-050's audit/traceability layer ships — proactive retrieval without an
     auditable trail is exactly the failure mode RUBICON warns against.

---

## 5. Implementation Roadmap (Cross-Reference)

| Layer | Recommendation | TD ID | Status | Owner Hint |
|-------|----------------|-------|--------|------------|
| Document chunking | SDP + Entanglement Index (§1.1) | TD-043 | Pending | Document/ingest team |
| Hybrid Search | Projection Fusion as tradeoff option (§1.2) | TD-044 | Done — needs benchmark + correct positioning | Search team |
| Compression | Dual-Use Embedding flag (§1.3) | TD-051 (new) | Pending — integration story | SDK team |
| Graph | RGL Modular RAG (§2.1) | TD-045 | Pending | Graph team |
| Graph | GraphWalk Tool — single-step variant (§2.2) | TD-046 | Partial | Graph + SDK team |
| Optimizer | Evolutionary Planner + measured fitness (§3.1) | TD-047 | Partial — commit + measure | Query team |
| Optimizer | AV-SQL 3-agent decomposition (§3.2) | TD-048 | Pending | Query team |
| Optimizer | Block-Batched Semantic Joins (§3.3) | TD-049 (renamed) | Partial | Query team |
| Memory/Architecture | RUBICON / AQL audit layer (§4.1) | TD-050 (renamed) | Pending — spec first | Architecture |
| Memory | Proactive retrieval — speculative (§4.2) | TD-052 (new) | Speculative — no citation | (none) |

---

## 6. Sequencing Guidance

If you are picking up Phase 5 work, do this order:

1. **Commit the skeleton** — `src/query/unified/evolutionary.rs` is currently untracked.
2. **Land Projection Fusion benchmarks** (§1.2) with **correct positioning** (RRF default,
   Projection as tradeoff). Closes TD-044.
3. **GraphWalk REST + tool exposure** (§2.2) — small, high-visibility; closes TD-046.
4. **Evolutionary planner with measured fitness** (§3.1) — sub-deliverable 1 of TD-047.
5. **EI metric tool** (§1.1, sub-deliverable 1) — read-only analyzer, ships independently.
6. **RGL** (§2.1) — depends on stable §2.2 surface.
7. **SDP preprocessing pipeline** (§1.1, sub-deliverable 2).
8. **RUBICON spec** (§4.1) — block on this for AV-SQL (§3.2), since the audit story needs to
   exist before LLM-driven query rewriting becomes safe.
9. **AV-SQL** (§3.2) and **Block-Batched Semantic Joins** (§3.3) — both depend on a stable
   `[llm]` config plus budget-safe LLM call infrastructure.
10. **Dual-Use Embedding flag** (§1.3) — passive integration story; can land any time.

Each step has its own benchmark/acceptance gate. Do not advance until the current one passes.

---

## 7. Findings From Paper Verification

### Second pass — April 26, 2026 (deeper read of papers we shipped code for)

A second verification round fetched method-section content (not just abstracts) for the four
papers backing TD-044, TD-045, TD-046, TD-047 — i.e. the ones with code in tree. Material drift
between paper claims and shipped code:

| TD | Paper | Drift |
|----|-------|-------|
| **TD-044** | 2604.13728 (Prajapati 2026) | Paper's B5 is **vector-space pre-fusion** (BGE 768d + Achlioptas-projected SPLADE 30522d→768d, single dense vector for ANN). Our `FusionStrategy::Projection` is a **score-level** fusion of post-retrieval scalar scores. Same name, different layer. Quantitative results don't transfer. |
| **TD-045** | 2503.19314 (Li et al. 2025) | The 143× speedup is on the **graph-retrieval phase** vs NetworkX, achieved by C++ implementations + batching — NOT primarily by dynamic filtering. Our docs previously credited the boundary filter for the speedup; corrected. ProximaDB's graph engines already use Rust/C++ bindings, so the headline number doesn't directly transfer. |
| **TD-046** | 2604.01610 (Ghandi et al. 2026) | Paper exposes **4 tools** (`get_node_by_property`, `get_all_nearest_neighbors`, `get_unique_property_values`, `think`) and uses **strict sequential single-step** navigation (iteration cap 30). Our `graph_step` ≈ `get_all_nearest_neighbors`; `graph_walk` (bounded BFS) is **not a paper concept**. Property-search and unique-values primitives unimplemented as agent tools. |
| **TD-047** | 2602.10387 (Erol et al. 2026) | Paper's mutation operator is **GPT-5 producing RFC-6902 JSON Patches** with two specific patch types (join-side selection, join reordering). Plans flattened from DAG to node list (~10× compression) before being sent to LLM. Fitness = min latency over 50 sandboxed runs. DBPlanBench: 240 queries (TPC-H + TPC-DS) on Apache DataFusion. 4.78× speedup is on a TPC-DS cross-channel sales query. Our skeleton uses random topological-order swaps — **not the paper's contribution**, correctly tagged sub A vs sub B. |

The TD register has been updated with these corrections. Module-level rustdocs in
`src/core/search/hybrid/fusion.rs` and `src/graph/rag/mod.rs` carry honest "paper attribution"
sections so future readers don't repeat the conflation.

**Note on arXiv:2604.17677 (Disentanglement / TD-043)**: the HTML version 404'd on second
fetch and the abstract does not expose the EI formula. Our EI implementation in
`src/analytics/entanglement.rs` documents itself as a *defensible operationalization*
(within-vs-cross-topic cosine ratio) rather than a faithful paper reproduction. A future
follow-up that reads the full PDF should reconcile.

### First pass — April 25, 2026 (initial verification)

All 9 cited arXiv papers were fetched on 2026-04-25 and confirmed to exist. **However, the
auto-generated V3 summaries were inaccurate in five technically meaningful ways:**

| Paper | V3 claimed | Reality |
|-------|-----------|---------|
| 2604.17677 (Loghmani) | "Centroid + residual topic vectors in AXIS index" | Document **preprocessing pipeline** + Entanglement Index metric; the index is unchanged |
| 2604.13728 (Prajapati) | "Projection fusion is 33% faster AND more diverse" (implicit: better) | RRF wins **relevance** (nDCG@10 = 0.828); B5 wins **speed and diversity only** — strict tradeoff |
| 2604.21413 (Stonebraker et al.) | "Context Lake managed by DBOS principles" | **RUBICON / AQL** — query language + architecture for auditable agent plans; no DBOS, no memory store |
| 2510.08489 (Trummer) | "Evolutionary sampling, O(n+m) LLM calls" | **Block nested loops with batched prompts** — different algorithm |
| 2602.10387 (Erol et al.) | (TD-047 description correct on technique) | Yes, but novelty is **LLM-as-mutation-operator**; our skeleton uses random mutations (still useful, but not the paper's contribution) |

Implications for the older TD-043..TD-050 entries written before paper verification: those
entries should be re-read in light of the table above. The current TECHNICAL_DEBT.adoc has been
updated to match the verified findings (April 25, 2026 entry).

The "PASK" acronym in the V3 draft had **no paper backing** — it was synthesized by the
auto-pipeline. TD-052 is now flagged as speculative.

---

_Last updated_: 2026-04-25 (post arXiv verification)
_Companion to_: `docs/10-quality/TECHNICAL_DEBT.adoc` (TD-043..TD-052)
_See also_: `docs/_internal/roadmap/STRATEGIC_ROADMAP.adoc` Phase 5 section
