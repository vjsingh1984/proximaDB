//! Discovery refinement passes (Phase 8 F1).
//!
//! Each pass is dispatched by [`DiscoveryJobKind`] and receives a
//! [`PassContext`] (the pinned snapshot + capability handles). The executor
//! owns orchestration (pin → begin_publish → run pass → commit/abort); a pass
//! owns only the refinement logic, so adding a pass is implementing one
//! function plus one match arm — the executor never changes.
//!
//! `dedup` is the first real pass. `recluster` / `re_embed` / `quality` /
//! `trajectory` are identity passes (republish unchanged) until implemented;
//! per the plan refinement the next real pass is `Recluster`. A pass whose
//! required capability is absent also returns an identity result rather than
//! failing the job.

mod context;
pub mod dedup;

pub use context::PassContext;

use anyhow::Result;

use crate::services::discovery::job::{DiscoveryJobKind, DiscoveryJobResult};

/// Dispatch the refinement pass for `kind` against `ctx`.
///
/// Not-yet-implemented kinds resolve to an identity pass: the executor then
/// republishes the pinned snapshot unchanged, exercising the full
/// pin → publish → atomic-switch path without altering data.
pub(crate) async fn run(
    kind: DiscoveryJobKind,
    ctx: &PassContext,
) -> Result<DiscoveryJobResult> {
    match kind {
        DiscoveryJobKind::Dedup => dedup::run(ctx).await,
        DiscoveryJobKind::Recluster
        | DiscoveryJobKind::ReEmbed
        | DiscoveryJobKind::QualityScan
        | DiscoveryJobKind::TrajectoryAnalysis => Ok(DiscoveryJobResult::default()),
    }
}
