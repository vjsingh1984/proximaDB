//! Predicate-shortfall diagnostic (TD-064).
//!
//! Lives in the foundation `search-types` crate so that transport-neutral
//! search DTOs (e.g. `proximadb_runtime::rich_search::RichSearchResponse`) can
//! reference it without an upward dependency on the modality-tier
//! observability engine. The observability engine re-exports it from its
//! historical `search_plan_trace` module path so existing callers keep
//! compiling.

use serde::{Deserialize, Serialize};

/// TD-064: Diagnostic block describing a predicate-aware recall shortfall.
///
/// Emitted when ANN returned a candidate pool, the metadata filter trimmed
/// it, and the survivor count is below `requested_k`. Clients should treat
/// this as a correctness signal — either re-issue with `PreFilter` mode,
/// widen the filter, or accept the disclosed shortfall.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct PredicateShortfall {
    /// The `top_k` value the caller asked for.
    pub requested_k: u32,
    /// The number of results actually returned after predicate filtering.
    pub returned_k: u32,
    /// Pool size considered before the predicate (oversample budget).
    pub oversample_pool: u32,
    /// AnnFilteringMode that produced this shortfall (`post_filter`,
    /// `inline`, or `pre_filter`). Free-form string so callers can encode
    /// catalog `AnnFilteringMode` variants without coupling.
    pub ann_filtering_mode: String,
}
