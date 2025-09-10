# Proto v1 Migration Checklist — Current Status

This checklist tracks the cutover from legacy `proximadb` proto to `proximadb.v1` across code, APIs, and build.

## Status Summary (2025-09-10)
- ✅ API paths use v1 types across vector/graph/collections/SQL
- ✅ HashMap-based metadata structures adopted
- ✅ v1 schema expanded (e.g., quantization, collection types)
- ✅ Legacy proto removed from build and `src/proto/proximadb.rs` deleted.
- ✅ Performance work tracked separately; avoid unverified claims here

## Action Items
1) Services (vectors) ✅ COMPLETED
- [x] Add v1 result builders and v1 unified search with hints
- [x] Switch v1 search to v1 cache methods  
- [x] v1 batch/get return v1 responses directly
- [x] Convert VectorOperationsService to use v1 types exclusively
- [x] Migrate UnifiedHandlers to use v1 types exclusively

2) Core search ✅ COMPLETED
- [x] Add `to_search_vector_record_v1`
- [x] Migrate core/conversions.rs to v1 types
- [x] Implement v1 filter conversion

3) Collections ✅ COMPLETED
- [x] Migrate `services/collection/manager.rs` to v1 `Collection*` types
- [x] Convert Collections over gRPC to return v1 via converters

4) Cache ✅ COMPLETED
- [x] Add v1 get/put wrappers
- [x] Switch API-facing cache users to prefer v1 wrappers

5) Edges audit ✅ COMPLETED
- [x] REST vector endpoints call v1 UnifiedHandlers
- [x] gRPC vector endpoints call v1 UnifiedHandlers
- [x] Collections gRPC returns v1 Collections
- [x] Remove duplicate handler files (handlers_new.rs, backup files)

6) API Layer Migration ✅ COMPLETED
- [x] Migrate src/services/operations/vectors.rs v1-heavy paths to v1 builders
- [x] Migrate src/api_handlers/unified_handlers.rs to v1 types
- [x] Migrate src/network/rest/v1/handlers.rs to v1 types
- [x] Migrate src/network/grpc/collection_service.rs to v1 types
- [x] Migrate src/network/multi_server.rs to v1 types

7) Remaining Tasks (updated 2025-09-10)
- [x] Remove `proto/proximadb.proto` from `build.rs`
- [x] Delete `src/proto/proximadb.rs` after confirming no references
- [x] Verify storage layer does not rely on legacy message shapes

## Validation Commands
- Check for legacy imports: `rg -n "crate::proto::proximadb::" src` (should return no results)
- Check build inputs: `rg -n "proximadb.proto" build.rs` (should return no results)

## How To Measure Progress
- Count remaining legacy references:
  - `rg -n "crate::proto::proximadb::" src`
- Verify build references:
  - `rg -n "proximadb.proto" build.rs`
- Ensure gRPC + REST use v1:
  - `rg -n "handle_vector_.*_v1|proximadb_v1" src/network`

## Notes
- Migration is complete.