//! External Collection (Phase 8 F5 / TD-090).
//!
//! Register a lake table (Parquet) **un-copied** and build a ProximaDB-owned
//! vector index over it that serves through the same retrieval path as native
//! data — "One Data. One Index. No duplicated storage." The source stays
//! externally governed (`CatalogAuthorityMode::FederatedRead`); ProximaDB owns
//! only the index, not the records.
//!
//! Slice 1 (this module): register + build the IVF index in place + search.
//! Deferred (Slice 2+): BM25 inverted index, federated full-record fetch,
//! staleness-on-source-advance (`RebuildRequired`), Iceberg/Lance.
//!
//! See `docs/12-design/PHASE8_CONTINUOUS_LOOP_HLD_LLD_2026_05_28.adoc` (F5).

mod registry;
mod service;
mod source_reader;
mod types;

pub use registry::ExternalCollectionRegistry;
pub use service::{
    ExternalCollectionService, ExternalHit, RefreshOutcome, EXTERNAL_INDEX_PROJECTION,
};
pub use types::{
    ExternalCollection, ExternalCollectionSpec, ExternalCollectionStatus, ExternalFormat,
};
