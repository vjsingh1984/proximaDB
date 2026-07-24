// Copyright 2025 ProximaDB
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.

//! Unified search-parameters cluster — hoisted from `proximadb::core::search`
//! (root-crate decomposition, gap 1).
//!
//! This is the cohesive `UnifiedSearchParams` cluster: the parameter struct
//! itself + its leaf-type dependencies (`ProgressiveRecalls`, `SearchMode`,
//! `SearchEffort`, `VectorFreshnessMode`, `HybridSearchMode`, `BlockPruneConfig`,
//! `FilterOptimizationHints`) + the `SearchParams` backward-compat alias.
//! Every member depends only on foundation types
//! (`FilterExpression`/`ComparisonOperator`, `DistanceMetric`,
//! `UnifiedQuantizationLevel`, `BlockPruneMode`) and `std`/`serde`/`serde_json`
//! — no root-internal orchestration types, no concrete engine references.
//! Hoisting it clears the `crate::core::search::SearchParams` reference from
//! `src/storage/traits` (gap-1 blocker of the root-crate decomposition).
//!
//! The old import paths are preserved via `pub use` re-export shims in the
//! root crate's `src/core/search/mod.rs` so every existing caller resolves
//! unchanged. The conversion-function modules (`protocol_conversions`,
//! `filter_extraction`) stay in the root — they are not types and reference
//! proto types not needed in the data struct.

use std::collections::HashMap;

use proximadb_distance_kernel::DistanceMetric;
use proximadb_filter_expression::{ComparisonOperator, FilterExpression};
use proximadb_quantization_model::UnifiedQuantizationLevel;

use crate::block_prune::BlockPruneMode;

/// Custom recall rates for progressive search stages
#[derive(Debug, Clone)]
pub struct ProgressiveRecalls {
    /// Target recall for binary quantization stage
    pub binary_recall: Option<f32>,
    /// Target recall for INT8 quantization stage
    pub int8_recall: Option<f32>,
    /// Target recall for product quantization stage
    pub pq_recall: Option<f32>,
}

/// Search mode for controlling accuracy vs speed tradeoff (LanceDB-inspired IVF optimization)
///
/// This enum allows users to choose between exact search (100% recall) and
/// approximate search (faster but potentially lower recall).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum SearchMode {
    /// Exact search with 100% recall — searches all partitions and
    /// **disables block-level centroid pruning** at request time
    /// (since 2026-05-30: `SstEngine::fallback_to_direct_search`
    /// auto-sets `BlockPruneConfig::force_exact = true` when
    /// `SearchMode::Exact` is requested, even if the caller's
    /// `block_prune` config didn't explicitly opt in).
    ///
    /// Use this for accuracy-critical applications where the
    /// performance cost of a full block scan is acceptable.
    Exact,

    /// Approximate search using IVF-style partition pruning.
    /// Searches only `nprobe` closest partitions for faster queries.
    /// - `nprobe`: Number of partitions to search (None = auto-calculate as sqrt(num_partitions))
    /// - Typical recall: 95-98% with nprobe=sqrt(n)
    Approximate {
        /// Number of IVF partitions to probe (None = auto-calculate)
        nprobe: Option<usize>,
    },

    /// Adaptive mode: automatically selects Exact or Approximate based on dataset size.
    /// - Uses Exact for small datasets (< threshold vectors)
    /// - Uses Approximate for large datasets
    Adaptive {
        /// Vector count threshold above which approximate search is used
        threshold: usize,
    },
}

/// Default per-query vector-count cap for the default `Adaptive` mode. The
/// *primary* exact-vs-approximate gate is a dim-aware byte budget at the SST
/// search route (`EXACT_SCAN_MAX_BYTES`, see
/// `docs/12-design/EXACT_VS_ANN_ROUTING_COST_MODEL_2026_06_26.adoc`); this count
/// cap bounds very-low-dimension collections where bytes stay small. A caller can
/// request a tighter cap (or `Exact`/`Approximate`) explicitly.
pub const DEFAULT_ADAPTIVE_VECTOR_THRESHOLD: usize = 100_000;

impl Default for SearchMode {
    /// TD-165: the default search is **cost-adaptive** — exact when a full segment
    /// scan is cheap (one ranged GET; bytes = `N·dim·4`), approximate otherwise.
    /// `Exact` (strict 100% recall) and `Approximate` remain explicit opt-ins. This
    /// replaced the former strict-`Exact` default so the orchestrated index path no
    /// longer has to be paid on small collections (and, conversely, large
    /// collections are not forced through a full brute-force scan by default).
    fn default() -> Self {
        SearchMode::Adaptive {
            threshold: DEFAULT_ADAPTIVE_VECTOR_THRESHOLD,
        }
    }
}

impl SearchMode {
    /// Create approximate search mode with auto-calculated nprobe
    pub fn approximate() -> Self {
        SearchMode::Approximate { nprobe: None }
    }

    /// Create approximate search mode with specific nprobe value
    pub fn approximate_with_nprobe(nprobe: usize) -> Self {
        SearchMode::Approximate {
            nprobe: Some(nprobe),
        }
    }

    /// Create adaptive mode with default threshold (10,000 vectors)
    pub fn adaptive() -> Self {
        SearchMode::Adaptive { threshold: 10_000 }
    }

    /// Check if this is exact search mode
    pub fn is_exact(&self) -> bool {
        matches!(self, SearchMode::Exact)
    }

    /// Calculate the effective nprobe value for a given number of partitions
    pub fn effective_nprobe(&self, num_partitions: usize, dataset_size: usize) -> usize {
        match self {
            SearchMode::Exact => num_partitions, // Search all partitions
            SearchMode::Approximate { nprobe } => {
                nprobe.unwrap_or_else(|| {
                    // LanceDB-style: sqrt(num_partitions) for ~95% recall
                    3.max((num_partitions as f32).sqrt().ceil() as usize)
                })
            }
            SearchMode::Adaptive { threshold } => {
                if dataset_size < *threshold {
                    num_partitions // Use exact for small datasets
                } else {
                    // Use approximate for large datasets
                    3.max((num_partitions as f32).sqrt().ceil() as usize)
                }
            }
        }
    }

    /// Map this mode to a per-query [`SearchEffort`] for the AXIS warm path.
    ///
    /// - `Exact` ⇒ `Exact` (keeps the index's recall-maximizing default).
    /// - `Approximate { nprobe }` ⇒ `Approximate { hint: nprobe }`.
    /// - `Adaptive` ⇒ `None`: the index's own size-aware default already
    ///   adapts to dataset size.
    pub fn to_search_effort(&self) -> Option<SearchEffort> {
        match self {
            SearchMode::Exact => Some(SearchEffort::Exact),
            SearchMode::Approximate { nprobe } => Some(SearchEffort::Approximate { hint: *nprobe }),
            SearchMode::Adaptive { .. } => None,
        }
    }
}

/// Per-query search effort derived from [`SearchMode`], threaded into the AXIS
/// query so the warm HNSW/IVF path honors the accuracy-vs-latency knob.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SearchEffort {
    /// Maximize recall: keep the index's full / size-aware effort.
    Exact,
    /// Trade recall for latency. `hint` is the caller's explicit budget
    /// (HNSW `ef` / IVF `nprobe`); `None` asks the engine for a default.
    Approximate {
        /// Explicit per-query effort budget (HNSW `ef` / IVF `nprobe`).
        hint: Option<usize>,
    },
}

impl SearchEffort {
    /// Recall-trading HNSW `ef` floor for `Approximate { hint: None }`, as a
    /// multiple of `top_k`.
    const APPROX_EF_TOPK_MULT: usize = 2;
    /// Absolute floor so tiny `top_k` still explores enough candidates.
    const APPROX_EF_FLOOR: usize = 64;

    /// HNSW per-query `ef` override. `None` ⇒ keep the index's own size-aware
    /// default (today's recall-maximizing behavior — used for `Exact`).
    pub fn hnsw_ef_override(&self, top_k: usize) -> Option<usize> {
        match self {
            SearchEffort::Exact => None,
            SearchEffort::Approximate { hint: Some(ef) } => Some((*ef).max(top_k)),
            SearchEffort::Approximate { hint: None } => {
                Some((top_k * Self::APPROX_EF_TOPK_MULT).max(Self::APPROX_EF_FLOOR))
            }
        }
    }

    /// IVF per-query `nprobe` given the configured `nlist` (partition count).
    pub fn ivf_nprobe(&self, nlist: usize) -> usize {
        match self {
            SearchEffort::Exact => nlist.max(1),
            SearchEffort::Approximate { hint: Some(n) } => (*n).clamp(1, nlist.max(1)),
            SearchEffort::Approximate { hint: None } => {
                1.max((nlist as f32).sqrt().ceil() as usize)
            }
        }
    }
}

/// Vector search freshness mode controlling the consistency/cost trade-off
/// for routes that read from a per-collection vector object-economy directory.
///
/// - `Strong` (default): always merge the WAL/memtable delta so the search sees
///   every record committed via the canonical WAL.
/// - `BoundedStale { max_staleness_ms }`: accept directory state up to
///   `max_staleness_ms` old before merging.
/// - `StaleOk`: skip the WAL delta read entirely (cheapest).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub enum VectorFreshnessMode {
    /// Default: WAL/memtable delta is always merged.
    #[default]
    Strong,
    /// Accept directory state up to `max_staleness_ms` old before merging.
    BoundedStale {
        /// Maximum acceptable staleness in milliseconds.
        max_staleness_ms: u64,
    },
    /// Skip the WAL delta read entirely.
    StaleOk,
}

impl VectorFreshnessMode {
    /// True when the search path MUST merge the WAL/memtable delta for
    /// this mode (i.e. `Strong`, or `BoundedStale` whose bound has been
    /// exceeded — the bound check is the caller's responsibility).
    pub fn requires_delta_merge(&self) -> bool {
        !matches!(self, Self::StaleOk)
    }

    /// Stable lowercase mode name used in EXPLAIN payloads and trace
    /// fields. Kept separate from the `Display` impl so external surfaces
    /// don't accidentally pin on the human-readable form.
    pub fn explain_label(&self) -> &'static str {
        match self {
            Self::Strong => "strong",
            Self::BoundedStale { .. } => "bounded_stale",
            Self::StaleOk => "stale_ok",
        }
    }

    /// Decide whether the search path should scan the WAL/memtable
    /// delta for records committed after the directory watermark.
    ///
    /// LSN-only variant: equivalent to
    /// [`Self::should_scan_delta_with_time`] with `watermark_ns = 0` and
    /// `now_ns = 0`. For `BoundedStale` this conservatively treats
    /// the time bound as "unknown" → scan when newer. Use the
    /// `_with_time` variant when the directory's `freshness_watermark_ns`
    /// and wall-clock are available so `BoundedStale` can actually skip
    /// the scan within its bound.
    ///
    /// Pure function — no I/O, no allocation, fully unit-testable.
    pub fn should_scan_delta(&self, current_lsn: u64, watermark_lsn: u64) -> bool {
        self.should_scan_delta_with_time(current_lsn, watermark_lsn, 0, 0)
    }

    /// Decide whether to scan, with the directory's `freshness_watermark_ns`
    /// and the current wall-clock available. Rules:
    ///
    /// * `StaleOk` → false (cheapest read; never scans).
    /// * `Strong` → true iff `current_lsn > watermark_lsn`. Time inputs
    ///   are ignored.
    /// * `BoundedStale { max_staleness_ms }`:
    ///   1. If `current_lsn <= watermark_lsn`, there's nothing newer to
    ///      merge → false (regardless of bound).
    ///   2. Else, if `watermark_ns` and `now_ns` are both positive and
    ///      `(now_ns - watermark_ns) / 1_000_000 < max_staleness_ms`,
    ///      the directory is fresher than the caller's bound → false
    ///      (accept stale read).
    ///   3. Otherwise → true (catch up via WAL scan).
    ///
    /// When `watermark_ns == 0` or `now_ns == 0` the bound check is
    /// skipped — treat as "time unknown," conservatively scan. This
    /// matches the writer's current placeholder of emitting
    /// `freshness_watermark_ns = 0` when no real timestamp source is
    /// wired.
    ///
    /// Pure function — no I/O, no allocation, fully unit-testable.
    pub fn should_scan_delta_with_time(
        &self,
        current_lsn: u64,
        watermark_lsn: u64,
        watermark_ns: i64,
        now_ns: i64,
    ) -> bool {
        if matches!(self, Self::StaleOk) {
            return false;
        }
        // When `current_lsn == 0` the global manifest's LSN allocator has not
        // been advanced. This happens when the WAL writer path adds records
        // to the memtable without going through `manifest::append_*` (the
        // current v2 INSERT path in `write_vector_batch_native_arc_with_mode`).
        // In that case LSN-based gating would silently hide unflushed records
        // from search: `0 <= watermark_lsn` is true for any watermark, so the
        // delta scan would be skipped even when memtable has data.
        //
        // Treat `current_lsn == 0` as "tracking unavailable" and fall through
        // to scan — the memtable lookup is cheap when empty. Only short-circuit
        // when we have evidence the watermark already covers the WAL.
        // Reconciled 2026-05-28 with the v2 INSERT→SEARCH gap.
        if current_lsn > 0 && current_lsn <= watermark_lsn {
            return false;
        }
        if let Self::BoundedStale { max_staleness_ms } = self {
            // Both timestamps must be valid for the bound check to be
            // meaningful. When the writer hasn't wired a real ns source
            // yet (watermark_ns == 0), conservatively scan.
            if watermark_ns > 0 && now_ns >= watermark_ns {
                let age_ms = ((now_ns - watermark_ns) / 1_000_000) as u64;
                if age_ms < *max_staleness_ms {
                    return false;
                }
            }
        }
        true
    }
}

/// Hybrid search mode controlling how BM25 text and vector results are combined
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize, Default)]
pub enum HybridSearchMode {
    /// Vector search only (default, ignores text_query)
    #[default]
    VectorOnly,
    /// Keyword/BM25 search only (ignores query vectors)
    KeywordOnly,
    /// Hybrid: combine BM25 + vector results using Reciprocal Rank Fusion
    Hybrid,
    /// Hybrid with custom RRF k parameter (default k=60)
    HybridCustom {
        /// Reciprocal Rank Fusion k parameter (higher = more uniform weighting)
        rrf_k: u32,
    },
}

/// Hints to guide the filter optimizer for better execution plans
#[derive(Debug, Clone, Default)]
pub struct FilterOptimizationHints {
    /// Expected fraction of rows that will pass the filter (0.0 to 1.0)
    pub expected_selectivity: Option<f64>,
    /// Name of the preferred index to use for this filter
    pub preferred_index: Option<String>,
    /// Whether parallel execution is permitted
    pub allow_parallel: bool,
}

/// Configuration for block-level centroid pruning.
///
/// # Recall vs latency trade-off
///
/// Block-centroid pruning scans only a subset of SSTable blocks based
/// on each block's centroid distance to the query. This is fast
/// (constant work regardless of N) but assumes blocks are spatially
/// organized — records inside a block share spatial locality, and
/// the centroid is a meaningful "summary" of the block.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct BlockPruneConfig {
    /// Disable all block-level pruning (force brute-force scan).
    ///
    /// **Auto-set to `true` when `SearchMode::Exact` is requested**
    /// (since 2026-05-30) — see `SstEngine::fallback_to_direct_search`.
    /// Before that change, `SearchMode::Exact` silently allowed
    /// sqrt-mode centroid pruning to drop recall to 5% at scale.
    pub force_exact: bool,
    /// Pruning mode: "sqrt" (default), "ratio", or "fixed".
    pub mode: BlockPruneMode,
    /// Ratio of blocks to keep when mode == Ratio (0.0–1.0).
    pub ratio: f32,
    /// Minimum number of blocks to keep.
    pub min_keep: usize,
    /// Maximum number of blocks to keep (0 = no cap).
    pub max_keep: usize,
    /// Override the minimum blocks threshold for pruning.
    /// When set, bypasses the production MIN_BLOCKS_FOR_PRUNING (100) threshold.
    /// Use `Some(0)` in tests to always apply pruning regardless of block count.
    /// None = use production default (100 blocks).
    #[serde(default)]
    pub min_blocks_override: Option<usize>,
    /// TD-RDSTRAT-5 lever-3: weight `k` on the per-block radius in the prune score
    /// `d(query, centroid) − k·radius` (a distance lower bound). `0.0` = rank by
    /// raw centroid distance (legacy). `> 0` favours spread-out blocks that could
    /// still hold a near neighbour, raising recall at a fixed keep-ratio. Calibrated
    /// for L2/Euclidean (radius and distance share units); leave `0.0` for cosine.
    #[serde(default)]
    pub radius_k: f32,
}

impl Default for BlockPruneConfig {
    fn default() -> Self {
        Self {
            force_exact: false,
            mode: BlockPruneMode::Sqrt,
            ratio: 0.2,
            min_keep: 1,
            max_keep: 0,
            min_blocks_override: None,
            radius_k: 0.0,
        }
    }
}

impl BlockPruneConfig {
    /// Create a config for testing that bypasses the MIN_BLOCKS_FOR_PRUNING threshold.
    /// Always applies pruning logic regardless of block count.
    ///
    /// Marked `doc(hidden)` because it is a test convenience, but it is NOT
    /// `#[cfg(test)]`-gated: downstream crates' `#[cfg(test)]` code calls it,
    /// and the crate's own `#[cfg(test)]` does not propagate to them.
    #[doc(hidden)]
    pub fn for_testing() -> Self {
        Self {
            min_blocks_override: Some(0),
            ..Default::default()
        }
    }
}

/// Unified search parameters for all storage engines
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct UnifiedSearchParams {
    // Core search parameters
    /// Query vectors for similarity search (supports single or batch search)
    pub query_vectors: Option<Vec<Vec<f32>>>,

    /// Single query vector (alternative to query_vectors for single queries)
    pub vector: Option<Vec<f32>>,

    /// Number of results to return
    pub top_k: Option<usize>,

    /// Distance metric to use for similarity calculation
    #[serde(skip)]
    pub distance_metric: Option<DistanceMetric>,

    /// Unified metadata filter expression supporting AND, OR, NOT operators
    pub filter_expression: Option<FilterExpression>,

    /// Legacy filters field for backward compatibility
    pub filters: Option<HashMap<String, serde_json::Value>>,

    /// Accuracy threshold for search (0.0-1.0)
    pub accuracy_threshold: Option<f32>,

    /// Include expired vectors in results
    pub include_expired: Option<bool>,

    /// Search timeout in milliseconds
    pub timeout_ms: Option<u64>,

    /// Enable two-stage search with quantization
    pub enable_two_stage: Option<bool>,

    /// Enable vectorized execution using Arrow compute kernels (TD-041)
    /// When enabled, uses batch processing for 10x faster predicate evaluation
    pub enable_vectorized_execution: Option<bool>,

    /// Enable parallel morsel processing (TD-039)
    /// When enabled, divides work into 4096-row morsels for parallel processing
    pub enable_parallel_morsels: Option<bool>,

    /// Enable pipeline-based execution with DataChunks (TD-031)
    /// When enabled, uses pull-based pipeline with selection vectors for zero-copy operations
    pub enable_pipeline_execution: Option<bool>,

    // Optional optimization hints
    /// Preferred quantization level for search
    #[serde(skip)]
    pub quantization_hint: Option<UnifiedQuantizationLevel>,

    /// Hint to enable/disable cluster optimization
    pub enable_clustering_hint: Option<bool>,

    /// Runtime optimization hints for search strategy selection
    #[serde(skip)]
    pub runtime_hints: Option<FilterOptimizationHints>,

    /// Hint to enable/disable metadata filtering optimization
    pub enable_metadata_filtering_hint: Option<bool>,

    /// Custom optimization parameters
    pub custom_hints: Option<HashMap<String, serde_json::Value>>,

    /// Vector Object Economy freshness mode. `None` means "use the
    /// service-layer default", which is currently
    /// [`VectorFreshnessMode::Strong`] — every search merges the WAL
    /// delta to honor canonical-WAL durability. Callers opt out
    /// explicitly via `Some(BoundedStale {..} | StaleOk)`.
    pub freshness_mode: Option<VectorFreshnessMode>,

    /// Internal: Indicates if the query requires ordering (e.g., gRPC/REST always true, SQL with ORDER BY true)
    pub requires_ordering: Option<bool>,

    // Progressive search parameters
    /// Enable progressive quantization-aware search
    pub enable_progressive_search: Option<bool>,

    /// Custom recall rates for progressive stages
    #[serde(skip)]
    pub progressive_recalls: Option<ProgressiveRecalls>,

    /// Search mode for accuracy vs speed tradeoff (LanceDB-inspired IVF optimization)
    /// Defaults to Exact (100% recall). Use Approximate for faster queries with ~95-98% recall.
    pub search_mode: SearchMode,

    /// Block-level pruning configuration (applies to SST/HELIX/SWIFT engines)
    pub block_prune: BlockPruneConfig,

    // Hybrid search parameters (BM25 + vector)
    /// Text query for BM25 keyword search (used in hybrid mode)
    pub text_query: Option<String>,

    /// Hybrid search mode: how to combine vector and text results
    pub hybrid_mode: HybridSearchMode,

    /// Weight for vector scores in hybrid fusion (0.0-1.0, default 0.5)
    pub vector_weight: Option<f32>,
}

impl Default for UnifiedSearchParams {
    fn default() -> Self {
        Self {
            query_vectors: None,
            vector: None,
            top_k: Some(10),
            distance_metric: Some(DistanceMetric::Cosine),
            filter_expression: None,
            filters: None,
            accuracy_threshold: Some(0.95),
            include_expired: Some(false),
            timeout_ms: Some(5000),
            enable_two_stage: Some(true),
            enable_vectorized_execution: Some(false), // Disabled by default (TD-041)
            enable_parallel_morsels: Some(false), // Intra-file parallelism (TD-039); inter-file is config-driven (search_parallel_files)
            enable_pipeline_execution: Some(false), // Disabled by default (TD-031)
            quantization_hint: None,
            enable_clustering_hint: Some(true),
            enable_metadata_filtering_hint: Some(true),
            custom_hints: None,
            requires_ordering: None,
            runtime_hints: None,
            enable_progressive_search: Some(false),
            progressive_recalls: None,
            search_mode: SearchMode::default(), // cost-adaptive by default (TD-165); exact when cheap
            block_prune: BlockPruneConfig::default(),
            text_query: None,
            hybrid_mode: HybridSearchMode::default(),
            vector_weight: None,
            freshness_mode: None,
        }
    }
}

impl UnifiedSearchParams {
    /// Return the freshness mode the search path should honor for this
    /// request. Unset → [`VectorFreshnessMode::Strong`] (the safe
    /// default). The accessor exists so service-layer callers do not
    /// have to repeat the unwrap-or-default at every read site.
    pub fn effective_freshness_mode(&self) -> VectorFreshnessMode {
        self.freshness_mode.clone().unwrap_or_default()
    }

    /// Create search params for a single vector query
    pub fn single_vector(query_vector: Vec<f32>) -> Self {
        Self {
            query_vectors: Some(vec![query_vector]),
            ..Default::default()
        }
    }

    /// Create search params for batch vector query
    pub fn batch_vectors(query_vectors: Vec<Vec<f32>>) -> Self {
        Self {
            query_vectors: Some(query_vectors),
            ..Default::default()
        }
    }

    /// Get the first query vector (for single vector search)
    pub fn first_query_vector(&self) -> Option<&Vec<f32>> {
        self.query_vectors.as_ref()?.first()
    }

    /// Check if this is a batch search
    pub fn is_batch_search(&self) -> bool {
        self.query_vectors.as_ref().is_some_and(|v| v.len() > 1)
    }

    /// Create a filter expression from simple key-value pairs
    pub fn with_simple_filters(mut self, filters: HashMap<String, serde_json::Value>) -> Self {
        if filters.is_empty() {
            return self;
        }

        let conditions: Vec<FilterExpression> = filters
            .into_iter()
            .map(|(key, value)| FilterExpression::Comparison {
                field: key,
                operator: ComparisonOperator::Equals,
                value,
            })
            .collect();

        let filter_expr = if conditions.len() == 1 {
            conditions
                .into_iter()
                .next()
                .unwrap_or(FilterExpression::And(Vec::new()))
        } else {
            FilterExpression::And(conditions)
        };

        // Combine with existing filter if present
        self.filter_expression = match self.filter_expression {
            Some(existing) => Some(FilterExpression::And(vec![existing, filter_expr])),
            None => Some(filter_expr),
        };

        self
    }
}

/// Backward-compatibility alias for [`UnifiedSearchParams`].
///
/// Use [`UnifiedSearchParams`] in new code — this alias exists to keep
/// pre-disambiguation imports (`use crate::core::search::SearchParams`)
/// working during the migration window. Other SearchParams structs in
/// this codebase (`RpcSearchParams`, `NetworkSearchParams`,
/// `ComputeSearchParams`, `AnnBenchSearchParams`) are independent types
/// with different fields and are not aliased.
pub type SearchParams = UnifiedSearchParams;

#[cfg(test)]
mod tests {
    use super::*;

    // ── VectorFreshnessMode ─────────────────────────────────────────────

    #[test]
    fn vector_freshness_mode_defaults_to_strong() {
        assert_eq!(VectorFreshnessMode::default(), VectorFreshnessMode::Strong);
    }

    #[test]
    fn unified_search_params_default_freshness_is_strong() {
        let params = UnifiedSearchParams::default();
        // Field unset on default — the safe default is provided by the accessor.
        assert!(params.freshness_mode.is_none());
        assert_eq!(
            params.effective_freshness_mode(),
            VectorFreshnessMode::Strong
        );
    }

    #[test]
    fn vector_freshness_mode_strong_requires_delta_merge() {
        assert!(VectorFreshnessMode::Strong.requires_delta_merge());
        assert!(
            VectorFreshnessMode::BoundedStale {
                max_staleness_ms: 5_000,
            }
            .requires_delta_merge()
        );
        assert!(!VectorFreshnessMode::StaleOk.requires_delta_merge());
    }

    #[test]
    fn vector_freshness_mode_explain_label_is_stable() {
        assert_eq!(VectorFreshnessMode::Strong.explain_label(), "strong");
        assert_eq!(
            VectorFreshnessMode::BoundedStale {
                max_staleness_ms: 1_000,
            }
            .explain_label(),
            "bounded_stale"
        );
        assert_eq!(VectorFreshnessMode::StaleOk.explain_label(), "stale_ok");
    }

    #[test]
    fn vector_freshness_mode_round_trips_through_json() {
        for mode in [
            VectorFreshnessMode::Strong,
            VectorFreshnessMode::BoundedStale {
                max_staleness_ms: 2_500,
            },
            VectorFreshnessMode::StaleOk,
        ] {
            let json = serde_json::to_string(&mode).expect("serialize");
            let decoded: VectorFreshnessMode = serde_json::from_str(&json).expect("deserialize");
            assert_eq!(decoded, mode);
        }
    }

    // ── should_scan_delta decision logic ────────────────────────────────

    #[test]
    fn should_scan_delta_skips_for_stale_ok_regardless_of_lsns() {
        // StaleOk MUST never trigger a scan, even when the WAL has
        // newer data than the directory watermark.
        assert!(!VectorFreshnessMode::StaleOk.should_scan_delta(/*now*/ 100, /*wm*/ 10));
        assert!(!VectorFreshnessMode::StaleOk.should_scan_delta(0, 0));
        assert!(!VectorFreshnessMode::StaleOk.should_scan_delta(u64::MAX, 0));
    }

    #[test]
    fn should_scan_delta_skips_strong_when_watermark_matches_or_exceeds_lsn() {
        // Watermark already covers all committed writes — nothing to merge.
        assert!(!VectorFreshnessMode::Strong.should_scan_delta(/*now*/ 50, /*wm*/ 50));
        assert!(!VectorFreshnessMode::Strong.should_scan_delta(/*now*/ 50, /*wm*/ 60));
    }

    #[test]
    fn should_scan_delta_triggers_strong_when_wal_has_newer_records() {
        assert!(VectorFreshnessMode::Strong.should_scan_delta(/*now*/ 100, /*wm*/ 50));
        assert!(VectorFreshnessMode::Strong.should_scan_delta(/*now*/ 1, /*wm*/ 0));
    }

    #[test]
    fn should_scan_delta_scans_strong_when_lsn_tracking_is_zero() {
        // Reconciled 2026-05-28: when `current_lsn == 0` the global manifest
        // LSN allocator hasn't been advanced (e.g. v2 INSERT path skips the
        // manifest::append_* call). Returning false here would silently
        // hide memtable records from search. Strong/BoundedStale must scan;
        // StaleOk continues to skip.
        assert!(VectorFreshnessMode::Strong.should_scan_delta(/*now*/ 0, /*wm*/ 0));
        assert!(
            VectorFreshnessMode::BoundedStale {
                max_staleness_ms: 5_000,
            }
            .should_scan_delta(0, 0)
        );
        assert!(!VectorFreshnessMode::StaleOk.should_scan_delta(0, 0));
    }

    #[test]
    fn should_scan_delta_treats_bounded_stale_like_strong_when_ns_unset() {
        // LSN-only entry point passes ns=0/0 → bound check is skipped,
        // BoundedStale falls back to Strong's behaviour.
        let mode = VectorFreshnessMode::BoundedStale {
            max_staleness_ms: 5_000,
        };
        assert!(mode.should_scan_delta(100, 50));
        assert!(!mode.should_scan_delta(50, 50));
    }

    // ── should_scan_delta_with_time (BoundedStale bound) ────────────────

    const MS_NS: i64 = 1_000_000;

    #[test]
    fn time_bound_stale_ok_always_skips_regardless_of_lsn_or_time() {
        assert!(!VectorFreshnessMode::StaleOk.should_scan_delta_with_time(
            100,
            10,
            1_000_000_000,
            1_000_000_000
        ));
    }

    #[test]
    fn time_bound_strong_scans_when_lsn_ahead_ignoring_time() {
        assert!(VectorFreshnessMode::Strong.should_scan_delta_with_time(
            100,
            50,
            1_000_000_000,
            1_000_000_000
        ));
    }

    #[test]
    fn time_bound_bounded_stale_skips_when_within_bound() {
        // watermark is 1000ms old, bound is 5000ms → within bound → skip.
        let mode = VectorFreshnessMode::BoundedStale {
            max_staleness_ms: 5_000,
        };
        let watermark_ns = 1_000_000_000_000; // t0
        let now_ns = watermark_ns + 1_000 * MS_NS; // t0 + 1000ms
        assert!(!mode.should_scan_delta_with_time(100, 50, watermark_ns, now_ns));
    }

    #[test]
    fn time_bound_bounded_stale_scans_when_beyond_bound() {
        // watermark is 10000ms old, bound is 5000ms → beyond bound → scan.
        let mode = VectorFreshnessMode::BoundedStale {
            max_staleness_ms: 5_000,
        };
        let watermark_ns = 1_000_000_000_000; // t0
        let now_ns = watermark_ns + 10_000 * MS_NS; // t0 + 10000ms
        assert!(mode.should_scan_delta_with_time(100, 50, watermark_ns, now_ns));
    }

    #[test]
    fn time_bound_bounded_stale_skips_when_lsn_caught_up() {
        let mode = VectorFreshnessMode::BoundedStale {
            max_staleness_ms: 5_000,
        };
        assert!(!mode.should_scan_delta_with_time(50, 50, 1_000_000_000_000, 1_000_000_000_000));
    }
}
