# ProximaDB Recommendations — Phase 5 Agentic Intelligence

**Refined**: 2026-05-08 (manual review + arXiv verification of implemented Phase 5 features)
**Verification**: All 9 cited arXiv papers verified and implemented/plumbed in the April 2026 sprint.
**Status**: Authoritative companion to `docs/10-quality/TECHNICAL_DEBT.adoc` Phase 5 (TD-043..TD-052)

---

## 1. Vector & Document Layer

### 1.1 Semantic Disentanglement via Document Preprocessing
- **Status**: **Done**.
- **TD entry**: `TD-043` (HIGH).
- **Implementation**:
  - `src/analytics/entanglement.rs` — Entanglement Index (EI) library.
  - `src/storage/document/sdp.rs` — 4-stage Semantic Disentanglement Pipeline (SDP).
  - `src/network/rest/v1/analytics.rs` — `POST /entanglement` and `GET /collections/{id}/entanglement`.
- **Outcome**: EI measurable for any collection; SDP integrated as optional ingestion transform.

### 1.2 Projection-Based Fusion (B5) — Speed/Diversity Tradeoff
- **Status**: **Done**.
- **TD entry**: `TD-044` (MEDIUM).
- **Implementation**:
  - `src/core/search/hybrid/fusion.rs` — `Projection` strategy implemented.
  - `docs/05-concepts/hybrid-fusion.adoc` — Updated with RRF vs. Projection positioning.
  - `benches/bench_21_hybrid_search_fusion.rs` — Benchmarked: RRF (248µs) wins relevance; Projection (255µs) wins diversity.
- **Outcome**: RRF remains default; Projection available as opt-in for diversity-focused workloads.

### 1.3 Unified Retrieval + Compression Embedding
- **Status**: **Done**.
- **TD entry**: `TD-051` (MEDIUM).
- **Implementation**:
  - Proto `CollectionConfig` — added `enable_dual_use_embeddings` flag (tag 24).
  - Python SDK — `DualUseStore` helper class and `DualUseModel` Protocol.
- **Outcome**: System supports models where embeddings serve both retrieval and context.

---

## 2. Graph Layer

### 2.1 Modular Graph RAG (RGL)
- **Status**: **Done**.
- **TD entry**: `TD-045` (CRITICAL).
- **Implementation**:
  - `src/graph/rag/mod.rs` — `NodeRetriever`, `SubgraphBuilder`, `NodeFilter` traits + `RagPipeline`.
  - `src/graph/rag/engine_impls.rs` — `VectorNodeRetriever`, `KHopSubgraphBuilder`, `LlmNodeFilter` (LLM-based pruning).
  - `src/network/rest/v1/graph.rs` — `POST /api/v1/graph/graphs/{id}/rag` endpoint.
  - `tests/graph_rag_integration_test.rs` — Verified end-to-end.
- **Outcome**: High-performance modular RAG pipeline with interior LLM-based pruning for token efficiency.

### 2.2 Tool-Based Navigation (GraphWalk)
- **Status**: **Done**.
- **TD entry**: `TD-046` (HIGH).
- **Implementation**:
  - `src/graph/service_traversal_api.rs` — `graph_walk` (BFS) and `graph_step` (single-step) primitives.
  - `src/network/rest/v1/graph.rs` — REST handlers for walk and step.
  - `clients/python/src/proximadb_sdk/integrations/mcp_tools.py` — MCP agent tools schema.
- **Outcome**: Agents can traverse graph structures via atomic tool calls, reducing context bloat for large graphs.

---

## 3. Multi-modal & Query Optimization

### 3.1 Evolutionary Query Planning with LLM-Proposed Edits
- **Status**: **Done**.
- **TD entry**: `TD-047` (MEDIUM).
- **Implementation**:
  - `src/query/unified/evolutionary.rs` — `EvolutionaryOptimizer` with genetic search and `llm_mutate` operator.
  - `src/query/unified/plan_execution_cache.rs` — Measured wall-time fitness caching.
  - `src/query/unified/optimizer.rs` — Async optimization stack plumbed through the engine.
- **Outcome**: Optimizer uses measured fitness (Sub A) and LLM-assisted mutations (Sub B) to find globally optimal join orders.

### 3.2 Agentic View Decomposition (AV-SQL)
- **Status**: **Done**.
- **TD entry**: `TD-048` (HIGH).
- **Implementation**:
  - `src/query/nl/mod.rs` — `AvSqlEngine` + `Rewriter`, `ViewGenerator`, `Composer` agent traits.
  - `src/network/rest/v1/nl.rs` — `POST /api/v1/nl/translate` endpoint.
- **Outcome**: 3-agent decomposition for complex multi-model NL queries, providing traceability via intermediate views.

### 3.3 Block-Batched Semantic Joins
- **Status**: **Done**.
- **TD entry**: `TD-049` (HIGH).
- **Implementation**:
  - `src/query/unified/executor.rs` — `execute_block_batch_semantic_join` with prompt-packing and LLM integration.
  - Feature-gated via `llm-joins`.
- **Outcome**: Efficient cross-model semantic joins using row-batching to minimize LLM API calls.

---

## 4. Agentic AI & Memory

### 4.1 RUBICON / AQL — Auditable Agentic Query Plans
- **Status**: **Done**.
- **TD entry**: `TD-050` (CRITICAL).
- **Implementation**:
  - `src/query/aql/` — `AqlQuery` AST and structured `AuditTrail` infrastructure.
  - `src/query/aql/executor.rs` — Coordinates sources and emits `AuditFrame`s.
  - `src/network/rest/v1/aql.rs` — `/execute` and `/audit/{id}` endpoints.
  - `src/query/unified/mod.rs` — `EXPLAIN VERBOSE` mapping to AQL plans.
- **Outcome**: Enterprise-grade auditable memory layer replacing opaque agent chains with deterministic query plans.

### 4.2 Proactive Agentic Memory (Speculative — No Citation)
- **Status**: **Pending — speculative**.
- **TD entry**: `TD-052` (LOW).
- **Recommendation**: Treat as future research direction.

---

## 5. Phase 6: Active Memory & Collective Reasoning (2026+)

### 5.1 True Memory Architecture (Encoding Gates)
- **Paper**: arXiv:2605.04897 (May 2026) — *"Storage Is Not Memory: A Retrieval-Centered Architecture for Agent Recall"*.
- **Concept**: Shifts from "Extraction at Ingestion" (discarding info) to "Verbatim Event Preservation" with an **Encoding Gate**. The gate scores incoming events for **Novelty, Salience, and Prediction Error**.
- **Value**: Ensures no potentially relevant info is lost before the query is known. Minimizes ingestion delay and cost.
- **Recommendation**: **TD-053** (CRITICAL). Implement the three-layer Encoding Gate in the document ingestion path.

### 5.2 L-RAG: Entropy-Based Lazy Loading
- **Paper**: arXiv:2601.06551 (Jan 2026) — *"L-RAG: Balancing Context and Retrieval with Entropy-Based Lazy Loading"*.
- **Concept**: Two-tier architecture: queries first process a compact summary. Expensive chunk retrieval is triggered only when model **predictive entropy** exceeds a threshold (calibrated uncertainty).
- **Value**: 26% retrieval reduction with minimal accuracy trade-off. 80-210ms latency savings per query.
- **Recommendation**: **TD-054** (HIGH). Add entropy-gating logic to the AQL executor and RUBICON orchestrator.

### 5.3 Memanto: Typed Semantic Memory
- **Paper**: arXiv:2604.22085 (April 2026) — *"Memanto: Typed Semantic Memory with Information-Theoretic Retrieval"*.
- **Concept**: Unified memory layer with 13 predefined categories, **Conflict Resolution** for contradictory memories, and temporal versioning. Uses "Information-Theoretic Vector Compression" for deterministic retrieval.
- **Value**: Surpasses hybrid graph-vector systems in accuracy (89.8%) with lower operational complexity.
- **Recommendation**: **TD-055** (HIGH). Define the 13 memory types in `proximadb-records` and implement the conflict resolution mechanism.

### 5.4 Agentic Hybrid Reference Architecture
- **Paper**: arXiv:2604.16394 (March 2026) — *"A Reference Architecture for Agentic Hybrid Retrieval"*.
- **Concept**: **Plan–Retrieve–Evaluate** loop with offline **Metadata Augmentation** (pseudo-queries). Uses multi-agent horizontal architecture for governance and auditability.
- **Value**: Reduces vocabulary mismatch between user intent and provider metadata. Provides compliance-grade retrieval traces.
- **Recommendation**: **TD-056** (HIGH). Implement the `PseudoQueryGenerator` for metadata and the horizontal multi-agent orchestrator.

---

_Last updated_: 2026-05-09
_Version_: 0.2.0
_Authors_: Vijaykumar Singh (verified from full arXiv corpus)
