//! # Async Ingest Scheduler (LLD §7)
//!
//! Single-process priority queue with five lanes (P0–P4) keyed on the LLD's
//! ingest taxonomy: sync commits hit P0, freshness-SLA work hits P1, hot
//! compaction P2, stats refresh P3, re-embed P4. Within a lane, work is
//! ordered FIFO so a single tenant can't starve another at the same
//! priority. A bounded fairness window guarantees lower-priority lanes
//! still drain even under sustained P0/P1 load.
//!
//! Phase 5's main responsibility is **populating field statistics** so the
//! Phase 1 SelectivityEstimator (currently fed an empty FieldStatistics)
//! has real numbers to plan against. `stats_refresh.rs` ships that
//! refresher; the scheduler routes the refresh tasks at P3.

pub mod async_scheduler;
pub mod stats_refresh;

pub use async_scheduler::{
    IngestPriority, IngestQueue, IngestTask, SchedulerConfig, SchedulerStats, TaskKind,
};
pub use stats_refresh::{FieldStatsRefresher, RefresherConfig};
