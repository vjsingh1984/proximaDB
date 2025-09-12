# ProximaDB Roadmap Status Reconciliation (2025-09-10)

## Executive Summary

Overall, major roadmap areas show strong progress with core implementations landed. This reconciliation aligns status with the current codebase.

Highlights:
- SQL frontend is the default path for REST/gRPC. It lowers SQL via sqlparser-rs and executes through the new planner/executor (VOS + Graph).
- SKS functions (SIMILAR/FOLLOW) are lowered and mapped to explicit operations (VectorSearch / GraphTraversal); Hybrid plans use RRF fusion.
- GROUP BY/HAVING are implemented (aggregate op) with basic numeric aggregation. JOIN is partially implemented: executor supports equality hash-join, but planner/executor Join fields are not yet aligned (enum mismatch), and complex joins remain unimplemented.
- Hybrid Orchestrator: Contracts stable, parallel vector+graph execution implemented, vector→graph seeding enabled; fusion maintained.
- SKS v1 (storage-coupled) wired as in-memory headers/embeddings with CSR relations and REST endpoints; ready to evolve to persistent engine-backed storage.
- Parameter binding has moved into the frontend (placeholders parsed and bound in planner); no SQL string substitution.
- Proto v1 migration is complete; legacy protos removed. HELIX engine implemented with tests and benchmarks.

## Status by Specification

1) Query SQL Alignment (docs/09-roadmap/in-progress/query_sql_alignment_consolidated.adoc)
 - Actual: Implemented and integrated; frontend is default.
 - Evidence: `src/query/sql_frontend/parser.rs` (placeholders, SKS validation), `src/query/execution/{mod,planner,executor}.rs` (ops + aggregate + join), `src/api_handlers/unified_handlers.rs` (frontend default)
 - New: GROUP BY/HAVING implemented; executor has equality hash-join; parameter binding in frontend; SIMILAR/FOLLOW mapped to ops.
 - Gaps: Align `ExecutionOperation::Join` between planner (left_key/right_key) and executor (parses `on` string); improve ON handling (qualified identifiers, multi-join), predicate/project pushdown, group-by expressions, UNION/CTE execution, streaming/pagination.

2) Proto v1 Migration (docs/09-roadmap/in-progress/proto_v1_migration_checklist.md)
- Actual: Complete. API paths use `crate::proto::proximadb_v1::*`; no `crate::proto::proximadb::*` usages in code paths.
- Evidence: `rg crate::proto::proximadb::` → none; `build.rs` does not compile legacy protos; `src/proto/proximadb.rs` is deleted.
- Gaps: None.

3) Semantic Knowledge Store (docs/09-roadmap/in-progress/semantic_knowledge_store_features.adoc)
 - Actual: In progress.
 - Evidence: gRPC `src/network/grpc/entity_service.rs`; REST `src/network/rest/v1/entities.rs`; storage `src/storage/entity_store.rs`.
 - New: In-memory headers/embeddings, CSR relations store, provenance registry; REST entity routes mounted; sql_frontend lowers SKS functions; planner emits explicit ops.
 - Gaps: Persistent engine-backed EntityStore; Graph→Vector seed handoff; text→embedding; temporal filters; end-to-end tests.

4) HELIX Engine (docs/04-storage-engines/helix.adoc)
- Actual: Implemented.
- Evidence: `src/storage/engines/impls/helix/*` with clustering, compaction, zone maps, tests and benchmarks.

5) Unified Operations Design / Technical Debt (various)
- Actual: Largely complete. The technical debt document was outdated and has been updated.

## Recommended Updates

- Adjust `query_sql_alignment_consolidated.adoc` to call out planner/executor Join alignment work and streaming/pagination gaps.
- Keep `proto_v1_migration_checklist.md` as Complete (verified via code search).
- Maintain SKS status as In Progress with explicit gaps (EntityStore persistence, temporal filters, E2E tests).

## Next Actions (Prioritized)

1) SKS Coupling v1 (P0)
   - Move EntityStore from in-memory to engine-backed persistence; add EntityID↔VectorID index and embedding catalog; enable Graph→Vector seed handoff path in the orchestrator.
2) Hybrid Orchestrator Enhancements (P0)
   - Add Graph→Vector seeding and fusion tuning; keep contracts stable; EXPLAIN improvements. Pagination/streaming optional (top-K typical).
3) Graph Engine Selection (P1)
   - Provide config/factory toggle to choose ORION (default), PULSAR (single-process), or QUASAR (single-process) for GraphService.
4) SKS End-to-End Tests (P1)
   - Persistence, temporal filters, provenance; hybrid flows covering SIMILAR → FOLLOW and FOLLOW → SIMILAR.

## Cache Orchestrator Integration Status (2025-09-11)

- Shared Parquet Metadata Cache & Prefetch: Complete. Footer cache with prefetch/warming implemented (`src/storage/engines/core/formats/columnar/footer_cache.rs`). I/O layer (`parquet_io_layer.rs`) tracks Metadata access via orchestrator; predictive prefetch queued via orchestrator.
- SharedContext Threading: Partial. `core::context::SharedContext` in place; Parquet I/O layer supports `new_with_context(.., ctx)`. Engines: VIPER exposes `with_context(..)`; NOVA uses shared columnar stack. Remaining: thread `SharedContext` into `UnifiedParquetReader` and pass from VIPER/NOVA constructors.
- SST Bloom/Meta Hooks & Provider Registration: Pending. `SstStorage` carries optional orchestrator; hook sites include decompression cache and manifest/meta reads. Next: implement `register_cache_provider` for `CacheType::FilterBitmap` and `CacheType::Metadata`, emit `track_access_async` at bloom/meta touchpoints.
