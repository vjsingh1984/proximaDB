# Proto v1 Migration Plan (In-Progress)

Scope: Complete migration from legacy `proto/proximadb.proto` and `src/proto/proximadb.rs` to namespaced v1 schemas under `proto/proximadb/v1/*.proto` with end-to-end alignment across gRPC, REST, and SQL.

## Reconciled Status (2025-09-09)
- API edges (gRPC/REST): v1-only for Vector (Search/Batch/Get) and Collection (mapping legacy→v1 in gRPC). Progressive REST uses v1 directly.
- Services: UnifiedHandlers expose v1-returning methods; VectorOperationsService produces v1 at source (unified_search_v1) and native for internals (unified_search_native).
- Internals: Streaming, Graph Hybrid, SQL Executor now consume native OptimizedSearchRecord (no proto in hot paths).
- Cache: v1 read/write wrappers added; legacy storage retained during cutover.
- Protos: v1 graph.proto fixed to use proximadb.v1.*; build.rs annotated for eventual legacy removal.
- Converters: legacy↔v1 for search results, metadata, vector records, and collections.

What changed since last review
- REST vector_search_with_metadata now returns v1 via UnifiedHandlers::handle_vector_search_v1.
- VectorOperationsService::unified_search_with_hints builds v1 first, caches via v1 wrapper, converts to legacy only for compatibility return.
- Confirmed gRPC VectorService uses only v1 wrappers; Collections gRPC converts legacy→v1 on create/get/list.
- Internals (streaming, graph hybrid, SQL executor) switched to unified_search_native (native-only hot paths).

Metrics snapshot (run locally to refresh)
- Legacy refs: `rg -n "crate::proto::proximadb::" src | wc -l` (by module: api_handlers≈35, network≈32, services≈41, core≈146, query≈3, graph≈5, storage≈537)
- v1 refs total: `rg -n "proto::proximadb_v1" src | wc -l` → ≈215

## Status Metrics (Auto‑Snapshot)
The following counts were gathered over the current tree to reflect real progress.

- Total source files under `src/`: 711
- Files still using legacy proto (`crate::proto::proximadb::`): 104
- Files using v1 proto (`proto::proximadb_v1`): 97
- Net migrated/native‑only files (no legacy reference): ~607 (711 − 104)

Per‑module file counts (legacy → v1)
- `src/api_handlers`: 0 legacy → 2 v1
- `src/network`: 0 legacy → 12 v1
- `src/services`: 2 legacy → 5 v1
- `src/core`: 19 legacy → 8 v1
- `src/query`: 2 legacy → 1 v1
- `src/graph`: 1 legacy → 12 v1
- `src/storage`: 65 legacy → 55 v1

Interpretation
- Edges (api_handlers/network) are effectively v1‑only now.
- Services are largely v1/native; a small number of legacy helpers remain for compatibility signatures.
- Core + Storage hold most of the remaining legacy usage and will be migrated next.

Metrics
- Remaining legacy references in repo (approximate, run locally): `rg -n "crate::proto::proximadb::" src` (services/API paths trending down; full removal pending).

Blocked/Deferred
- v1 filter conversion (awaiting v1 filter schema) — stub added in `core::search::protocol_conversions`.
- Full migration of Collection manager to v1 (pending v1 schema parity for quantization/index configs).

## Current Status (Snapshot)
- Protos: v1 present (`vector.proto`, `sql.proto`, `graph.proto`, types); build currently compiles legacy + v1 (legacy scheduled for removal).
- Rust modules: `src/proto/mod.rs` exposes both `proximadb` (legacy) and `proximadb_v1` (v1) during cutover.
- Usage: v1 widely used at API edges; internals use native domain types. Some modules still import `crate::proto::proximadb::*` (mainly storage/tests), to be removed post edge cleanup.

## Migration Objectives
- Internal core stays native; API edges use v1:
  - Core compute/storage returns native types (OptimizedSearchRecord, etc.).
  - UnifiedHandlers/Network convert to `proximadb.v1.*` at boundaries.
  - Temporary legacy adapters remain only where needed until removed.
- gRPC services: serve `proximadb.v1` for Vector/SQL/Graph; no legacy servers.
- REST: accept/emit v1 protobuf JSON; remove custom DTOs and legacy conversions.
- Build: remove legacy `proximadb.proto` from `build.rs` once code is migrated.

## Work Plan (Phases)
1) Proto cleanup
- Update `v1/graph.proto` to replace legacy imports and types with `proximadb.v1.*`.
- Regenerate descriptors; verify no v1 file depends on legacy.

2) Rust module cutover
- Replace `crate::proto::proximadb::*` with `crate::proto::proximadb_v1::*` across services, storage, and tests.
- Add minimal From/Into shims where necessary; then remove once call sites updated.
- Drop `pub mod proximadb;` from `src/proto/mod.rs` when unused.

3) API alignment
- gRPC: ensure Vector/SQL/Graph servers use v1 requests/responses only.
- REST: unify handlers to accept/return v1 types directly; delete bespoke DTOs.

4) Build and cleanup
- Remove `proto/proximadb.proto` from `build.rs` and rerun codegen.
- Delete `src/proto/proximadb.rs` after all references are gone.

## Tracking & Acceptance
- Definition of done: no `crate::proto::proximadb::` references; build.rs compiles only v1 files; hybrid graph queries use `proximadb.v1.VectorSearchRequest` and `proximadb.v1.SearchVectorRecord`.
- Risks: enum/value mismatches; JSON/prost serde gaps; ensure REST uses the same field names as v1.
- Next action: Fix `graph.proto` to remove legacy dependency, then begin replacing legacy imports in `src/services/operations` and `src/api_handlers/unified_handlers.rs`.

## Status Updates
- 2025-09-09: Updated `proto/proximadb/v1/graph.proto` to reference `proximadb.v1.*` and removed legacy import. Added shared conversions in `src/core/conversions.rs` for legacy→v1 and v1→legacy metadata and responses. Refactored `src/network/grpc/vector_service.rs` to use shared converters, reducing duplication and easing future cutover of `UnifiedHandlers` to v1. Added v1-returning wrappers in `UnifiedHandlers` and switched gRPC and REST v1 handlers to call them directly.
- 2025-09-09: Added v1 conversion helpers in `VectorOperationsService` (`optimized_results_to_proto_v1`, `optimized_to_proto_v1`) to enable emitting `proximadb_v1::SearchResult` at source. Next up: wire UnifiedHandlers search paths to use these for lower conversion overhead.
- 2025-09-09: Implemented `VectorOperationsService::unified_search_v1` to execute and emit v1 results (with legacy caching). Updated `UnifiedHandlers::handle_vector_search_v1` to call this path and build v1 responses directly.
- 2025-09-09: Converted UnifiedHandlers batch/get to v1-at-source: `handle_vector_batch_v1` now builds v1 response directly from WAL results; `handle_vector_v1` maps store result to v1 SearchResult without legacy roundtrip. gRPC/REST already call these.
- 2025-09-09: Added v1 VectorRecord conversion helpers and v1 getters in `VectorOperationsService` (`get_unflushed_vectors_v1`, `debug_list_all_unflushed_vectors_v1`). Preparing to replace remaining legacy proto usages in services and tests.
- 2025-09-09: Collection gRPC now maps legacy collections to v1 via converters (`legacy_collection_to_v1`). Added collection conversion helpers in `src/core/conversions.rs` for config/stats.
- 2025-09-09: Added v1 filter conversion stub (`from_proto_metadata_filter_v1_unimplemented`) in `core::search::protocol_conversions` to mark the API surface; actual conversion awaits v1 filter schema.
- 2025-09-09: REST progressive search handler now delegates directly to `UnifiedHandlers::handle_vector_search_v1`. IntegratedSearchOptimization reads from cache via new `get_if_fresh_v1` for v1 results (fallback to legacy), preserving compatibility while preferring v1.
- 2025-09-09: Added `VectorOperationsService::unified_search_with_hints_v1` to return `proximadb_v1::SearchResult` with hints. This complements existing legacy hints with a v1 path.
- 2025-09-09: Added `InternalSearchResult::to_search_vector_record_v1` to emit `proximadb_v1::SearchVectorRecord` directly; future call sites will transition to use this for v1 paths.
- 2025-09-09: REST `vector_search_with_metadata` updated to v1 (calls `handle_vector_search_v1`, returns `proximadb_v1::VectorOperationResponse`).
- 2025-09-09: Streaming, Graph Hybrid, SQL Executor now call `unified_search_native` and operate on `OptimizedSearchRecord` (no proto on hot paths).
- 2025-09-09: Vector `unified_search_with_hints` now builds v1, caches via v1, then converts to legacy for compatibility return.

## Near-Term Plan (Executes Until Clean)
1) Services (vectors): convert remaining v1-heavy builder usages to v1 builders only; keep legacy builders for compatibility signatures and remove later.
2) API handlers: ensure all live endpoints call v1-returning methods (REST/gRPC verified); avoid constructing legacy types in v1 paths.
3) Edges audit: remove or migrate backup/legacy REST handlers not wired to server routes.
4) Core filters: implement v1 filter conversion when v1 filter schema lands (stub exists).
5) Collections manager: migrate to v1 when v1 schema parity (quantization/index/storage) exists; until then, convert at edges.
