//! Multi-phase ranking integration.
//!
//! This module sits *above* the modality-layer rank crates
//! (`proximadb-rank-*`) and the query-runtime crate (`proximadb-query`),
//! providing the adapters that wire them together. It lives in the root
//! crate because both directions of the dep graph terminate here — the
//! workspace layering policy forbids `query-runtime → modality`, so the
//! integration glue belongs in a higher layer.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` (R-6).

pub mod cross_modal_adapter;
pub mod metrics;
pub mod orchestrator;

pub use cross_modal_adapter::CrossModalGlobalScorer;
pub use metrics::{PhaseScopedSink, RankMetrics};
pub use orchestrator::{RankRun, run_pipeline};
