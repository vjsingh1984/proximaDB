# Roadmap Reconciliation and Agent Actions (2025-09-11)

## Executive Summary
- Roadmap largely aligns with the codebase. SQL frontend, unified query planner/executor, and SKS v1 are implemented with clear next steps. Cache orchestrator exists and is partially wired through engines; SST hooks need completion. This document reconciles specs under `docs/09-roadmap/` with actual code and adds agent-ready tasks (paths, functions, tests) to drive completion.

## Scope Reviewed
- In-Progress: `query_sql_alignment_consolidated.adoc`, `hybrid_query_orchestrator.adoc`, `semantic_knowledge_store_features.adoc`, `unified_query_layer_alignment.adoc`, `cross_cache_orchestrator_wiring.adoc`, `engine_reader_cache_evaluation.adoc`, `proto_v1_migration_checklist.md`, `helix_engine_comprehensive_spec.adoc`.
- Planned: `UNIFIED_CACHE_ORCHESTRATOR_SPEC.adoc`, `FEATURE_GAPS_AND_FUTURE_ITEMS.adoc`, `ARCHITECTURE_SCALING_AND_HARDENING_PLAN.adoc`.

---

## SQL Frontend + Planner/Executor
Status
- Implemented and defaulted in handlers. Planner creates strategies and ops; executor supports vector, graph, hybrid, aggregates, and basic joins.
- Evidence: `src/api_handlers/unified_handlers.rs` (execute_sql_frontend), `src/query/sql_frontend/{parser.rs,lowering.rs}`, `src/query/execution/{planner.rs,executor.rs}`.

Gaps
- Join alignment: planner emits `Join { left_key,right_key }`, executor expects matching keys and input buffers. Multi-join/qualified identifiers incomplete.
- Relational strategy (pure relational), UNION/CTE, pagination/streaming finalization.

Agent Actions
- Join key extraction
  - Edit `src/query/execution/planner.rs`: implement `extract_join_keys` to parse qualified identifiers `a.id = b.entity_id`.
  - Ensure `left_alias/right_alias` map to `FROM` aliases; fall back to table names.
  - Acceptance: add unit test in `src/query/execution/tests.rs` covering inner/left join key parsing.
- Executor join consistency
  - Edit `src/query/execution/executor.rs::join_rows` to support qualified keys like `a.id` by stripping alias prefix; ensure stable key lookup.
  - Add test: join two buffers with `{a.id=b.id}` and `{a.id=b.entity_id}`.
- Pagination/streaming
  - Plan-level: support `limit/offset` post-fusion. Add optional `limit, offset` on `ExecutionPlan` or apply at end of each executor path.
  - Edit: tail of `execute_*_plan` to slice rows; ensure `GROUP BY` applies before slicing.
  - Tests: add small tests verifying limit/offset order.
- Tracking
  - Instrument `planner.rs` to include `optimizations`/`hints` already present; surface via `EXPLAIN` in `src/query/explain.rs`.

Verification
- Commands: `cargo test -p proximadb -- src/query/execution --verbose` and targeted join tests.

---

## Semantic Knowledge Store (SKS v1)
Status
- In-memory headers/embeddings with engine-backed write path via `VectorOperationsService`. CSR relations + provenance registry stubs; REST/gRPC endpoints wired.
- Evidence: `src/storage/entity_store.rs`, `src/network/rest/v1/{entities.rs,handlers.rs}`, `src/network/grpc/entity_service.rs`, hybrid executor seeding.

Gaps
- Persistent headers/search path, metadata/temporal filters, SKS search implementation, end-to-end tests.

Agent Actions
- Header persistence (engine-backed KV)
  - `src/storage/entity_store.rs`: replace `serde_json::to_string(&format!("{:?}", header))` with real serde struct; store via `StorageKV` (already injected `kv`). Add `get_header`/`put_header` helpers.
- Search implementation
  - Implement `search_entities` using `vector_engine` or `vector_service`: perform similarity search when `query_vector.is_some()`, apply `MetadataFilter` (map to `FilterExpression`), then hydrate to `Entity`.
- Temporal filters
  - Add `TemporalFilter` to proto when available; until then, gate behind feature flag and TODO.
- Orchestrator Hooks
  - Track header/embedding touches with `CrossCacheOrchestrator::pattern_tracker().track_access_async(.., CacheType::{EntityHeader,EmbeddingCatalog})` (parts already present).

Verification
- Add REST tests under `tests/` for `/v1/entities` CRUD and search; hybrid E2E using SQL SIMILAR→FOLLOW.
- Commands: `make test-integration` and targeted `cargo test --test graph_integration_test`.

---

## Hybrid Query Orchestrator
Status
- Concurrent vector+graph execution, seeding strategies, and RRF fusion implemented.
- Evidence: `src/query/execution/executor.rs` (hybrid path, seeding strategies), `src/query/execution/planner.rs` (Fusion op), `src/query/sks_extensions.rs`.

Gaps
- Configurable seeding (thresholds, weights), EXPLAIN clarity, wider test matrix.

Agent Actions
- Configurables
  - Add `hybrid.fusion.weights` and `hybrid.seeding.strategy` to `config/` + wire into planner/executor.
- EXPLAIN
  - Extend `src/query/explain.rs` to print fusion/seeding details; include operation costs.
- Tests
  - Add property tests for RRF weight effects and deterministic results.

Verification
- Commands: `cargo test -p proximadb src/query -- --nocapture`.

---

## Unified Cache Orchestrator and SharedContext
Status
- Orchestrator present with providers and pattern tracking; Footer cache implemented; SharedContext exists. Some wiring in engines, but SST bloom/meta hooks pending.
- Evidence: `src/storage/cache/orchestrator.rs`, `src/storage/engines/core/formats/columnar/footer_cache.rs`, `src/storage/engines/impls/nova/columnar_search.rs` (uses `UnifiedParquetReader`), `src/storage/engine.rs`/`SstStorage`.

Gaps
- Thread `SharedContext` through `UnifiedParquetReader`; register providers for `CacheType::{FilterBitmap,Metadata}` at SST layer; add `track_access_async` at bloom/meta touchpoints.

Agent Actions
- SharedContext threading
  - Add `new_with_context(ctx: &SharedContext)` constructor to `UnifiedParquetReader`; pass from VIPER/NOVA engine constructors.
  - Edit: `src/storage/engines/impls/nova/columnar_search.rs` and VIPER’s analogous reader to accept `Arc<UnifiedParquetReader>` with context.
- SST hooks
  - Edit: `src/storage/engines/impls/sst/*` to call orchestrator at bloom/meta read sites; implement provider registration via `register_cache_provider`.

Verification
- Add unit tests in `src/storage/cache/tests/*` to assert provider registration and access tracking; run `cargo test -p proximadb src/storage/cache`.

---

## Graph Engine Selection
Status
- Graph service present; multiple engines hinted by docs.

Agent Actions
- Implement factory toggle
  - Add config: `graph.engine = "ORION"|"PULSAR"|"QUASAR"` in `config/*.toml`.
  - Add `GraphService::from_config(&Config)` to select engine; wire in `UnifiedHandlers`.

Verification
- Add smoke tests spinning each engine; run targeted cargo tests.

---

## Proto v1 Migration
Status
- Complete. Only `crate::proto::proximadb_v1::*` paths used.

Verification
- `rg crate::proto::proximadb::` → none; keep checklist marked complete.

---

## PR & Validation Checklist (for all above)
- Code: minimal, focused changes matching module conventions; no `unwrap()`/`expect()` outside tests.
- Lint/format: `make fmt && make clippy`; Tests: `make test`.
- Docs: update `docs/09-roadmap/*` specs you modified with status lines and pointers.
- Benchmarks (if perf-impacting): `make benchmark` with before/after notes.

