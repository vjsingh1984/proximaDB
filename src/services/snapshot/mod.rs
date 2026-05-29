//! Snapshot pin + atomic republish (Phase 8 CS/CD foundation).
//!
//! This is the foundational substrate shared by the Continuous Discovery loop
//! (F1) and External Collection (F5): pin a read-only view of a collection's
//! canonical WAL position, then atomically republish a refined snapshot by
//! driving the collection's discovery projection through the catalog freshness
//! state machine (`Updating` -> `Fresh`).
//!
//! It is pure composition over existing primitives:
//! - the global WAL manifest singleton
//!   (`crate::storage::persistence::write_ahead_log::manifest`) for the snapshot
//!   position, and
//! - `proximadb_catalog::{CatalogProjection, ProjectionFreshnessState}` plus the
//!   established drop+create update pattern (see
//!   `CollectionService::upsert_collection_catalog_asset`) for atomic publish.
//!
//! See `docs/12-design/PHASE8_CONTINUOUS_LOOP_HLD_LLD_2026_05_28.adoc` (F1, S0).

mod coordinator;

pub use coordinator::{SnapshotPin, SnapshotPublishCoordinator, DISCOVERY_ACTIVE_PROJECTION};
