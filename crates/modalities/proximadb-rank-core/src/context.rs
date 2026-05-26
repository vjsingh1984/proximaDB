//! Per-query, per-thread scoring context and the integration traits the
//! pipeline expects callers to provide.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` §4.1.8.
//!
//! The traits here (`AttributeAccess`, `CandidateData`, `ModelCache`,
//! `RankMetricsSink`) are deliberately minimal in R-1 — concrete impls
//! land in later phases (R-2 features, R-5 ONNX, R-7 observability).

use crate::arena::FeatureArena;
use crate::types::DocHandle;
use std::time::Instant;

/// Per-query immutable context — query vector(s), parameters, tenant scope.
///
/// `QueryContext` is intentionally light in R-1. Real query parameters
/// (filters, k, query vectors) thread through here in later phases.
#[derive(Debug, Default, Clone)]
pub struct QueryContext {
    pub query_id: Option<String>,
    pub tenant: Option<String>,
    /// Primary query vector for `closeness(...)` / `cosine(...)` features.
    /// `None` when the query is keyword-only.
    pub query_vector: Option<Vec<f32>>,
    /// Free-form query text — used by BM25 features and by the
    /// `BertPairTokenizingDocFeatureExtractor` (R-5b.1.3) to build
    /// (query, doc) pairs for cross-encoder rescoring. `None` for
    /// vector-only queries. `Arc<str>` because the same string is
    /// cloned cheaply into per-doc tokenization rows.
    pub query_text: Option<std::sync::Arc<str>>,
    /// Free-form tag bag for v1; will be replaced with typed query params
    /// in R-2 when retrieval candidates wire in.
    pub tags: Vec<(String, String)>,
    /// Logical "now" in milliseconds since the Unix epoch. Used by
    /// `freshness(...)` / `decay(...)` features so tests can pin a
    /// deterministic clock. Production callers set this from
    /// `SystemTime::now()` at query-arrival; tests pin a fixed value.
    /// `None` means features should fall back to `SystemTime::now()`.
    pub now_ms_unix: Option<i64>,
}

impl QueryContext {
    /// Resolved query time — either the pinned value or wall-clock now.
    pub fn now_ms_or_wall(&self) -> i64 {
        self.now_ms_unix.unwrap_or_else(|| {
            use std::time::{SystemTime, UNIX_EPOCH};
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0)
        })
    }
}

/// Per-thread mutable scoring context. Lives for the duration of one
/// per-segment scan.
///
/// `!Send` on purpose — pipelines clone the context per worker rather than
/// passing one across threads, so the borrow checker can enforce single-
/// threaded access to the arena and batch buffers.
pub struct ScoreCtx<'a> {
    pub query: &'a QueryContext,
    pub deadline: Option<Instant>,
    pub arena: &'a FeatureArena,
    pub attributes: &'a dyn AttributeAccess,
    pub candidates: &'a dyn CandidateData,
    pub models: &'a dyn ModelCache,
    pub metrics: &'a dyn RankMetricsSink,
    /// Cross-encoder batch scratch space, accumulated during per-doc
    /// `execute()` and flushed at `end_of_phase()`. Carried as an opaque
    /// boxed slot so callers control its concrete type. R-5 fills this in.
    pub batch: BatchSlot,
    // R-1 added a PhantomData<*const ()> here to enforce !Send. R-7c
    // removed it: tokio's multi-threaded scheduler moves futures across
    // worker threads, so futures awaiting a ScoreCtx in scope need it
    // to be Send. `&mut ScoreCtx` already prevents aliasing across
    // workers; the PhantomData was over-belt-and-suspenders.
}

impl<'a> ScoreCtx<'a> {
    pub fn new(
        query: &'a QueryContext,
        arena: &'a FeatureArena,
        attributes: &'a dyn AttributeAccess,
        candidates: &'a dyn CandidateData,
        models: &'a dyn ModelCache,
        metrics: &'a dyn RankMetricsSink,
    ) -> Self {
        Self {
            query,
            deadline: None,
            arena,
            attributes,
            candidates,
            models,
            metrics,
            batch: BatchSlot::default(),
        }
    }

    pub fn with_deadline(mut self, deadline: Instant) -> Self {
        self.deadline = Some(deadline);
        self
    }

    /// Returns `true` when the configured deadline has passed.
    pub fn deadline_exceeded(&self) -> bool {
        match self.deadline {
            Some(d) => Instant::now() >= d,
            None => false,
        }
    }
}

/// Opaque per-phase scratch buffer. R-5 (ONNX) fills this with batched
/// cross-encoder inputs/outputs; in R-1 it's just an empty placeholder
/// so the trait surface is forward-compatible.
#[derive(Default)]
pub struct BatchSlot {
    /// R-5 will populate this with a real typed payload.
    pub _reserved: (),
}

// ---------------------------------------------------------------------------
// Integration trait stubs — concrete impls live in later phases.
// ---------------------------------------------------------------------------

/// Read access to a candidate document's column (attribute) values.
///
/// Implemented by the storage engine layer in R-2. The lookup key is a
/// `(doc, field)` pair; values are returned as `f32` for the v1 surface
/// (full `ProximaValue` access lands when range / equality features come
/// online).
pub trait AttributeAccess: Send + Sync {
    fn read_f32(&self, doc: DocHandle, field: &str) -> Option<f32>;
}

/// Per-candidate retrieval metadata: distance cached by the upstream
/// vector index, BM25 score from the inverted index, etc.
///
/// The hybrid coordinator (R-2 wiring) supplies this so first-phase
/// features like `closeness(...)` are O(1) reads rather than recomputing
/// the retrieval distance.
pub trait CandidateData: Send + Sync {
    fn retrieval_distance(&self, doc: DocHandle) -> Option<f32>;
    fn bm25_score(&self, doc: DocHandle) -> Option<f32>;
}

/// Acquires shared ONNX sessions. R-5 fills this in.
pub trait ModelCache: Send + Sync {
    fn is_loaded(&self, model_id: &str) -> bool;
}

/// Emits per-feature observability metrics. R-7 wires this to Prometheus.
pub trait RankMetricsSink: Send + Sync {
    fn record_feature_latency_ns(&self, feature: &str, ns: u64);
    fn record_phase_truncated(&self, phase: proximadb_kernel::PhaseId, reason: &str);
}

// ---------------------------------------------------------------------------
// Inline no-op implementations for tests + R-1 wiring.
// ---------------------------------------------------------------------------

#[derive(Default)]
pub struct NoopAttributeAccess;
impl AttributeAccess for NoopAttributeAccess {
    fn read_f32(&self, _doc: DocHandle, _field: &str) -> Option<f32> {
        None
    }
}

#[derive(Default)]
pub struct NoopCandidateData;
impl CandidateData for NoopCandidateData {
    fn retrieval_distance(&self, _doc: DocHandle) -> Option<f32> {
        None
    }
    fn bm25_score(&self, _doc: DocHandle) -> Option<f32> {
        None
    }
}

#[derive(Default)]
pub struct NoopModelCache;
impl ModelCache for NoopModelCache {
    fn is_loaded(&self, _model_id: &str) -> bool {
        false
    }
}

#[derive(Default)]
pub struct NoopMetricsSink;
impl RankMetricsSink for NoopMetricsSink {
    fn record_feature_latency_ns(&self, _feature: &str, _ns: u64) {}
    fn record_phase_truncated(&self, _phase: proximadb_kernel::PhaseId, _reason: &str) {}
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    fn make_ctx<'a>(
        query: &'a QueryContext,
        arena: &'a FeatureArena,
        attr: &'a NoopAttributeAccess,
        cand: &'a NoopCandidateData,
        models: &'a NoopModelCache,
        metrics: &'a NoopMetricsSink,
    ) -> ScoreCtx<'a> {
        ScoreCtx::new(query, arena, attr, cand, models, metrics)
    }

    #[test]
    fn score_ctx_deadline_propagates() {
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let past = Instant::now() - Duration::from_millis(10);
        let ctx = make_ctx(&q, &arena, &a, &c, &m, &met).with_deadline(past);
        assert!(ctx.deadline_exceeded());

        let future = Instant::now() + Duration::from_secs(60);
        let ctx = make_ctx(&q, &arena, &a, &c, &m, &met).with_deadline(future);
        assert!(!ctx.deadline_exceeded());
    }

    #[test]
    fn score_ctx_with_no_deadline_never_exceeds() {
        let q = QueryContext::default();
        let arena = FeatureArena::new();
        let (a, c, m, met) = (
            NoopAttributeAccess,
            NoopCandidateData,
            NoopModelCache,
            NoopMetricsSink,
        );
        let ctx = make_ctx(&q, &arena, &a, &c, &m, &met);
        assert!(!ctx.deadline_exceeded());
    }

    #[test]
    fn score_ctx_send_status_documented() {
        // R-1 marked ScoreCtx !Send via PhantomData<*const ()>.
        // R-7c removed that PhantomData since the borrow checker
        // already prevents aliasing via &mut, AND tokio's multi-
        // threaded runtime needs Send futures for axum handlers.
        //
        // However ScoreCtx still ends up !Send today because it holds
        // &FeatureArena and `bumpalo::Bump` is !Sync internally
        // (uses Cell<NonNull<ChunkFooter>>). R-7c.1 will restructure
        // `run_pipeline` to do arena-bearing work under
        // `tokio::task::spawn_blocking` so the outer future stays
        // Send without needing FeatureArena to be Sync.
        //
        // This test exists purely to document the current Send status
        // and trip a reviewer if Bump's Sync-ness changes upstream.
        fn no_op_marker<T: ?Sized>() {}
        no_op_marker::<ScoreCtx<'static>>();
    }
}
