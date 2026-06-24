//! Storage↔Index (AXIS) decoupling contract — segregated role traits (ISP) plus a
//! slim, storage-facing query DTO that resolves the "query-envelope crux" from
//! `docs/12-design/STORAGE_INDEX_COMPUTE_DECOUPLING_CONTRACTS_2026_06_21.adoc`.
//!
//! # Why this crate exists
//!
//! `src/storage` reaches the AXIS index through the concrete `AxisManager`, but a
//! read-only survey (against `develop`) showed storage actually touches it through
//! only a handful of methods — and the rich query envelope
//! (`AxisHybridQuery`/`AxisManagerQueryResult`) drags in `proximadb_catalog`,
//! `core::search`, and `observability` types that cannot live in a foundation crate.
//!
//! The resolution (contract doc §A2, option 1 — the preferred one) is a **slim
//! storage-facing DTO**: define exactly what storage passes/reads, and convert
//! to/from the rich envelope *at the index boundary* (in the root crate, where
//! `AxisManager` lives). This keeps the trait crate dependency-light and lets it
//! sit in the foundation layer.
//!
//! Measured facts that shaped the DTO (all verified against the storage call sites):
//! * storage sets only `collection_id`, `vector_query`, `metadata_filters`,
//!   `id_filters`, `top_k`, `include_expired`, and `search_effort` on the query;
//!   it never sets `ann_filtering_policy`, `estimated_selectivity`, or
//!   `ann_filtering_mode` (all defaulted at the boundary), so the DTO omits them;
//! * storage reads only `results` from the query result — the observability fields
//!   (`strategy_used`, `predicate_shortfall`, `selected_filtering_mode`,
//!   `quantized_route`) are consumed by the service layer, which calls
//!   `AxisManager` directly, so the result DTO omits them.
//!
//! # Depend on the role, not the god-object (ISP + DIP)
//!
//! Engines should hold `Arc<dyn IndexQuery>` (etc.) injected at construction; the
//! concrete `AxisManager` is one implementor. This crate only defines the
//! abstractions and the boundary DTO — the implementations and conversions live in
//! the root crate next to `AxisManager`, and the per-engine field narrowing
//! (`Arc<AxisManager>` → `Arc<dyn IndexQuery>`) is a separate, coordinated step.

use async_trait::async_trait;
use proximadb_records::ProximaRecord;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Slim storage-facing query DTO (§A2 resolution)
// ---------------------------------------------------------------------------

/// Storage-facing hybrid query — the slim mirror of the AXIS `AxisHybridQuery`
/// carrying only the fields storage actually sets. The root-crate boundary
/// converts this into the rich AXIS envelope (defaulting the catalog/selectivity/
/// mode fields storage never populates).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct IndexHybridQuery {
    /// Target collection for the query.
    pub collection_id: String,
    /// Optional vector-similarity component.
    pub vector_query: Option<IndexVectorQuery>,
    /// Metadata field filter predicates.
    pub metadata_filters: Vec<IndexMetadataFilter>,
    /// Exact vector-ID filters for point lookups.
    pub id_filters: Vec<String>,
    /// Maximum number of results to return.
    pub top_k: usize,
    /// Whether to include MVCC-expired records.
    pub include_expired: bool,
    /// Per-query accuracy-vs-latency intent. `None` keeps each index's own
    /// size-aware default (behavior-identical to no knob). The *mechanism*
    /// (HNSW `ef` / IVF `nprobe`) is applied at the AXIS boundary, not here.
    pub search_effort: Option<IndexSearchEffort>,
}

/// Vector-similarity component of an [`IndexHybridQuery`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum IndexVectorQuery {
    /// Dense full-precision query vector.
    Dense {
        /// Query vector in f32.
        vector: Vec<f32>,
        /// Minimum similarity score threshold.
        similarity_threshold: f32,
    },
    /// Sparse query vector (dimension-index → value).
    Sparse {
        /// Sparse query vector.
        vector: std::collections::HashMap<u32, f32>,
        /// Minimum similarity score threshold.
        similarity_threshold: f32,
    },
}

/// Metadata filter predicate.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexMetadataFilter {
    /// Metadata field to filter on.
    pub field: String,
    /// Comparison operator.
    pub operator: IndexFilterOperator,
    /// Value to compare against.
    pub value: serde_json::Value,
}

/// Comparison operators for [`IndexMetadataFilter`]. Mirrors the AXIS
/// `FilterOperator`; converted at the boundary.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum IndexFilterOperator {
    /// Exact equality.
    Equals,
    /// Inequality.
    NotEquals,
    /// Strictly greater than.
    GreaterThan,
    /// Greater than or equal.
    GreaterThanOrEqual,
    /// Strictly less than.
    LessThan,
    /// Less than or equal.
    LessThanOrEqual,
    /// Membership in a set.
    In,
    /// Exclusion from a set.
    NotIn,
    /// String contains substring.
    Contains,
    /// String starts with prefix.
    StartsWith,
    /// String ends with suffix.
    EndsWith,
    /// SQL-style LIKE pattern.
    Like,
    /// Inclusive `[lower, upper]` range.
    Between,
    /// Value is null or missing.
    IsNull,
    /// Value is present and non-null.
    IsNotNull,
}

/// Per-query search intent — the slim mirror of `core::search::SearchEffort`.
/// Carries only the *intent*; the per-index mechanism (`hnsw_ef_override` /
/// `ivf_nprobe`) is reconstructed at the AXIS boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum IndexSearchEffort {
    /// Maximize recall — keep the index's full / size-aware effort.
    Exact,
    /// Trade recall for latency. `hint` is the caller's explicit budget
    /// (HNSW `ef` / IVF `nprobe`); `None` asks the engine for a recall-trading
    /// default below the exact ceiling.
    Approximate {
        /// Explicit per-query effort budget.
        hint: Option<usize>,
    },
}

/// Storage-facing query result — only the scored hits storage reads. The rich
/// AXIS result keeps its observability fields for the service layer.
#[derive(Debug, Clone, Default)]
pub struct IndexQueryResult {
    /// Scored results ordered by relevance.
    pub results: Vec<IndexScoredResult>,
}

/// A single scored hit (with MVCC expiry).
#[derive(Debug, Clone)]
pub struct IndexScoredResult {
    /// Identifier of the matching vector.
    pub vector_id: String,
    /// Similarity score between the query and this result.
    pub similarity: f32,
    /// MVCC expiration timestamp, if the record has a TTL.
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

// ---------------------------------------------------------------------------
// Segregated role traits (ISP) — engines depend only on the role they use
// ---------------------------------------------------------------------------

/// Read path: execute a hybrid query against the index.
///
/// Implemented by `AxisManager`; held by every storage engine + Orion as the
/// search seam. This is the highest-traffic role — narrowing engine fields to
/// `Arc<dyn IndexQuery>` is the bulk of the DIP win.
#[async_trait]
pub trait IndexQuery: Send + Sync {
    /// Execute a hybrid query and return the scored hits.
    async fn query(&self, query: IndexHybridQuery) -> anyhow::Result<IndexQueryResult>;
}

/// Write path: index vectors produced by a storage flush.
#[async_trait]
pub trait IndexIngest: Send + Sync {
    /// Index the records emitted by a flush of `collection_id`; `files_created`
    /// names the SST/segment files the flush produced.
    async fn handle_flushed_vectors(
        &self,
        collection_id: &str,
        flushed_vectors: Vec<ProximaRecord>,
        files_created: Vec<String>,
    ) -> anyhow::Result<()>;
}

/// Metrics the storage warm-load path inspects.
#[async_trait]
pub trait IndexMetrics: Send + Sync {
    /// Number of vectors currently registered in the index for `collection_id`.
    async fn registered_vector_count(&self, collection_id: &str) -> usize;

    /// Whether the collection has an in-memory IVF index (for health diagnostics).
    async fn has_ivf_index(&self, collection_id: &str) -> bool;

    /// Whether the collection has a persisted IVF index on disk (for health diagnostics).
    async fn has_persisted_ivf_index(&self, collection_id: &str) -> bool;

    /// Cold-serving status for IVF collections: returns Some((state, loaded, total))
    /// where `state` is the serving phase, `loaded` is segments warm-loaded, and
    /// `total` is total segments. Returns None for non-IVF collections.
    async fn ivf_cold_serving_status(&self, collection_id: &str) -> Option<(String, usize, usize)>;
}

/// Maintenance hooks invoked by compaction / background optimization.
///
/// `get_collection_indexes` (returning per-index reader handles) is intentionally
/// *not* part of this contract yet: on `develop` it is a stub returning an empty
/// vec, and its return type (`Arc<dyn AxisVectorIndex>`) would force a shared
/// reader trait + supertrait wiring across the concrete index impls. It lands
/// with the compaction-reader follow-up, once compaction actually consumes it.
#[async_trait]
pub trait IndexMaintenance: Send + Sync {
    /// Rebuild a single named index for `collection_id`.
    async fn rebuild_index(&self, collection_id: &str, index_name: &str) -> anyhow::Result<()>;

    /// Analyze the collection and apply adaptive index optimizations.
    async fn analyze_and_optimize(&self, collection_id: &str) -> anyhow::Result<()>;

    /// Apply a live HNSW ef_search hot-swap for `collection_id`. Returns
    /// the outcome as JSON for admin surface consumption.
    async fn apply_hnsw_ef_hot_swap(
        &self,
        collection_id: &str,
        new_ef_search: u32,
    ) -> anyhow::Result<serde_json::Value>;

    /// Apply a live IVF nprobe hot-swap for `collection_id`. Returns
    /// the outcome as JSON for admin surface consumption.
    async fn apply_ivf_nprobe_hot_swap(
        &self,
        collection_id: &str,
        new_nprobe: u32,
    ) -> anyhow::Result<serde_json::Value>;
}

/// Collection lifecycle on the index side. Defined for completeness; storage
/// wiring is post-MVP (the contract doc marks lifecycle "define, don't wire").
#[async_trait]
pub trait IndexLifecycle: Send + Sync {
    /// Drop all index state for `collection_id`.
    async fn drop_collection(&self, collection_id: &str) -> anyhow::Result<()>;

    /// Suspend indexing/serving for `collection_id`.
    async fn suspend_collection(&self, collection_id: &str) -> anyhow::Result<()>;

    /// Resume a suspended collection. Returns `true` if it was suspended.
    async fn resume_collection(&self, collection_id: &str) -> anyhow::Result<bool>;

    /// Whether `collection_id` is currently suspended.
    async fn is_suspended(&self, collection_id: &str) -> bool;
}

/// Combined index-engine role traits for storage backends that need multiple roles
/// (e.g., SST needs query + ingest + metrics + lifecycle + maintenance). Implemented by `AxisManager`.
pub trait IndexEngine:
    IndexQuery + IndexIngest + IndexMetrics + IndexLifecycle + IndexMaintenance
{
    /// Downcast to `Any` for concrete-type access (e.g., AXIS-specific recall-target advisor methods).
    /// Callers can use `.downcast_ref::<AxisManager>()` to recover the concrete type.
    fn as_any(&self) -> &dyn std::any::Any;
}
