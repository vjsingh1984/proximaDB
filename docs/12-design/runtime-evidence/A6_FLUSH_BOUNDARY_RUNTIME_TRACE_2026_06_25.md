# A6 Flush-Boundary Runtime Trace (2026-06-25)

Empirical pin of the **live server vector-flush boundary** before wiring the A6
storage-write fence. Per CLAUDE.md #12 (no vibe-coding correctness-critical code)
and the convergence brief: a fence at a dead/shutdown boundary is worse than none,
so the live `do_flush` path was confirmed against a running server, not inferred
statically.

## Method
- Built `proximadb-server` offline (git index protocol; registry cache populated).
- Ran the real server binary (`-l debug`, `RUST_LOG=proximadb=debug`) — unlike the
  integration tests, the binary installs a tracing subscriber, so the existing
  funnel/coordinator log markers are observable.
- `POST /api/v2/collections` (sst, dim 8) → `POST .../records/batch` (500 vectors)
  → observe 8s (live) → signal → observe shutdown.

## Findings (conclusive)
1. **Live insert never reaches `do_flush`.** The 500-vector insert persists only to
   a WAL segment (`<coll>/wal/<batch>.bcwal`, bincode) + a manifest entry. No
   trait-`flush()` funnel marker, no coordinator marker, no `do_flush` during normal
   operation. On-disk after the run: WAL + manifest + lease only — **zero SST/data
   files**. Server durability is WAL-backed; materialization to storage does not
   happen on the live path.
2. **The server traps SIGINT only (`tokio::signal::ctrl_c`), not SIGTERM.** A
   `kill -TERM` bypasses graceful shutdown entirely (process dies, no flush). Only
   `kill -INT` runs `db.shutdown() → StorageEngine::stop() →
   flush_memtable_to_storage()`.
3. **The one live `do_flush` boundary is the SIGINT-shutdown coordinator — and it
   currently FAILS.** Graceful shutdown reaches
   `WALFlushCoordinator::flush_all_collections` → `execute_coordinated_flush`, finds
   the 500 unflushed vectors, then aborts:
   ```
   ⚠️ Coordinator: No collection service available, proceeding without metadata_info
   ❌ Failed to flush: "No storage engine specified for collection <id> and no metadata available"
   ✅ Flushed 0 collections, 0 vectors   ⚠️ 1 collections failed
   ```
   The throwaway coordinator (`engine.rs` `flush_memtable_to_storage`,
   `WALFlushCoordinator::new()`) has no `collection_service` and passes
   `preferred_engine=None, flush_context=None`, so it cannot resolve which engine to
   flush to. The boundary is both **shutdown-only** and **non-functional today**.

## Consequences for A6
- The fence must be enforced at the orchestration component that owns the lease
  manager (the coordinator flush path), **checked at the top — before metadata/engine
  resolution** — so it is exercised regardless of the downstream materialization gap
  and is correctly positioned for when that gap is fixed.
- `FlushParameters` is `Serialize/Deserialize`, so it cannot carry an
  `Arc<PartitionLeaseManager>`/`dyn` fence; the fence is injected as a storage-layer
  `StorageWriteFence` trait object from `shared_services`, keeping `storage`↔`cluster`
  decoupled.
- The server-materialization gap (coordinator engine/metadata resolution) is a
  **dependent TD**, tracked separately; the A6 fence does not depend on it to be
  correct, only to be exercised end-to-end on a successful server flush.
