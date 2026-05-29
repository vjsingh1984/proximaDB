//! Continuous Discovery loop (Phase 8 F1).
//!
//! A `DiscoveryJob` pins a read-only snapshot of a collection
//! (`crate::services::snapshot`), runs an offline refinement pass (dedup,
//! recluster, re-embed, quality scan, trajectory analysis), and atomically
//! republishes the refined snapshot back into serving via the
//! `SnapshotPublishCoordinator`. Serving reads the prior snapshot until the
//! atomic switch — no half-built state is ever visible.
//!
//! See `docs/12-design/PHASE8_CONTINUOUS_LOOP_HLD_LLD_2026_05_28.adoc` (F1).

mod executor;
mod job;
pub mod passes;
mod registry;
mod service;

pub use executor::{spawn_discovery_executor, DiscoveryJobExecutor, DEFAULT_POLL_INTERVAL};
pub use job::{DiscoveryJob, DiscoveryJobKind, DiscoveryJobResult, DiscoveryJobStatus};
pub use registry::DiscoveryRegistry;
pub use service::DiscoveryService;
