//! Continuous Discovery loop (Phase 8 F1).
//!
//! A `DiscoveryJob` pins a read-only snapshot of a collection
//! (`crate::services::snapshot`), runs an offline refinement pass (dedup,
//! recluster, re-embed, quality scan, trajectory analysis), and atomically
//! republishes the refined snapshot back into serving via the
//! `SnapshotPublishCoordinator`. Serving reads the prior snapshot until the
//! atomic switch — no half-built state is ever visible.
//!
//! The `DiscoveryTrigger` is the feedback arm: it turns serving-side signals
//! (recall degradation, freshness breaches, workload drift) into scheduled jobs
//! — what makes the loop continuous rather than operator-invoked.
//!
//! See `docs/12-design/PHASE8_CONTINUOUS_LOOP_HLD_LLD_2026_05_28.adoc` (F1).

mod drift;
mod executor;
mod job;
pub mod passes;
mod registry;
mod service;
mod trigger;

pub use drift::{
    DEFAULT_DRIFT_INTERVAL, DEFAULT_DRIFT_THRESHOLD_WRITES, DriftWatcher, drift_exceeds,
    interval_from_env, spawn_drift_watcher, threshold_writes_from_env,
};
pub use executor::{DEFAULT_POLL_INTERVAL, DiscoveryJobExecutor, spawn_discovery_executor};
pub use job::{DiscoveryJob, DiscoveryJobKind, DiscoveryJobResult, DiscoveryJobStatus};
pub use registry::DiscoveryRegistry;
pub use service::DiscoveryService;
pub use trigger::{DiscoveryTrigger, TriggerSignal};
