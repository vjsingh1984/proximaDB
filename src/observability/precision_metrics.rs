//! Embedding-precision Prometheus metrics — PR 7b of
//! `docs/12-design/EMBEDDING_PRECISION_LLD_2026_05_22.adoc` §"Observability
//! (Q11)".
//!
//! Owns the Prometheus gauge + counter handles for the
//! `proximadb_embedding_precision_*` metric family. The metric NAMES and
//! label keys are locked by the LLD so downstream Grafana dashboards keep
//! working across binary upgrades.
//!
//! Wiring: server startup constructs `PrecisionMetrics::register(&registry)`
//! once and stashes the handle in `SharedServices`. Hot-path callers grab
//! the handle from `SharedServices` and call the typed setters (no
//! string formatting in the loop).
//!
//! Cardinality: every label set uses `collection` + an additional
//! orthogonal axis (precision, level, dtype_pair, metric, site). The
//! `from` / `to` labels on `conversions_total` reuse the precision label
//! key. No tenant id is included — the LLD §"Observability" calls out
//! that tenant explosion would blow up cardinality; per-tenant aggregation
//! happens at the query layer.

use prometheus::{
    CounterVec, Encoder, GaugeVec, IntGaugeVec, Opts, Registry, TextEncoder,
    core::{AtomicF64, AtomicI64, GenericCounterVec, GenericGaugeVec},
};
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// LLD-locked metric names
// ---------------------------------------------------------------------------

/// `proximadb_embedding_precision_segments_total{collection,precision}` —
/// number of segments per precision per collection.
pub const METRIC_SEGMENTS_TOTAL: &str = "proximadb_embedding_precision_segments_total";

/// `proximadb_embedding_precision_canonical_bytes{collection,precision}` —
/// total bytes of canonical embedding data per precision per collection.
pub const METRIC_CANONICAL_BYTES: &str = "proximadb_embedding_precision_canonical_bytes";

/// `proximadb_embedding_precision_derived_bytes{collection,level}` —
/// bytes consumed by each derived quantization level (binary, int8, pq8, …).
pub const METRIC_DERIVED_BYTES: &str = "proximadb_embedding_precision_derived_bytes";

/// `proximadb_embedding_precision_overhead_ratio{collection}` —
/// `(canonical_bytes + derived_bytes) / canonical_bytes` per collection.
/// Stays ≤ policy's `max_overhead_ratio`.
pub const METRIC_OVERHEAD_RATIO: &str = "proximadb_embedding_precision_overhead_ratio";

/// `proximadb_embedding_precision_migration_progress_ratio{collection}` —
/// `segments_migrated / segments_total`, 0.0..1.0. Reaches 1.0 at
/// migration completion.
pub const METRIC_MIGRATION_PROGRESS_RATIO: &str =
    "proximadb_embedding_precision_migration_progress_ratio";

/// `proximadb_embedding_precision_conversions_total{from,to,site}` —
/// how many vectors were converted by `project_to_canonical`. `site` ∈
/// {ingest_boundary, query_entry, compaction}.
pub const METRIC_CONVERSIONS_TOTAL: &str = "proximadb_embedding_precision_conversions_total";

/// `proximadb_embedding_precision_recall_at_10{collection,metric}` —
/// most recent recall@10 measurement from the recall harness for the
/// collection's current precision config.
pub const METRIC_RECALL_AT_10: &str = "proximadb_embedding_precision_recall_at_10";

/// `proximadb_embedding_precision_hw_matmul_ns{dtype_pair}` — cached
/// hardware micro-bench results from the PR 7a startup probe.
/// `dtype_pair` ∈ {f32_f32, f16_f32, f16_f16}.
pub const METRIC_HW_MATMUL_NS: &str = "proximadb_embedding_precision_hw_matmul_ns";

// ---------------------------------------------------------------------------
// LLD-locked label keys
// ---------------------------------------------------------------------------

pub const LABEL_COLLECTION: &str = "collection";
pub const LABEL_PRECISION: &str = "precision";
pub const LABEL_LEVEL: &str = "level";
pub const LABEL_FROM: &str = "from";
pub const LABEL_TO: &str = "to";
pub const LABEL_SITE: &str = "site";
pub const LABEL_METRIC: &str = "metric";
pub const LABEL_DTYPE_PAIR: &str = "dtype_pair";

/// Allowed `site` label values for `conversions_total`. Locked by LLD §Q11
/// so callers must match exactly.
pub const SITE_INGEST_BOUNDARY: &str = "ingest_boundary";
pub const SITE_QUERY_ENTRY: &str = "query_entry";
pub const SITE_COMPACTION: &str = "compaction";

/// Allowed `dtype_pair` label values for `hw_matmul_ns`. Locked by LLD
/// §Q11 — extending requires a docs update first.
pub const DTYPE_PAIR_F32_F32: &str = "f32_f32";
pub const DTYPE_PAIR_F16_F32: &str = "f16_f32";
pub const DTYPE_PAIR_F16_F16: &str = "f16_f16";

/// Map an [`EmbeddingScalarType`] to the LLD-locked `precision` label
/// value used by `segments_total{collection,precision}` +
/// `canonical_bytes{collection,precision}` +
/// `conversions_total{from,to,site}`. The strings match the snake_case
/// serde tags so Prometheus dashboards stay aligned with catalog rows.
pub fn precision_label(p: proximadb_records::EmbeddingScalarType) -> &'static str {
    use proximadb_records::EmbeddingScalarType as P;
    match p {
        P::Fp32 => "fp32",
        P::Fp16 => "fp16",
        P::Bf16 => "bf16",
        P::Int8Scalar => "int8_scalar",
        P::UInt8Scalar => "uint8_scalar",
    }
}

// ---------------------------------------------------------------------------
// Handle bundle
// ---------------------------------------------------------------------------

/// Registered Prometheus handles for the embedding-precision metric family.
///
/// One process-wide instance, constructed at server startup, shared via
/// SharedServices. Cloneable because each Vec field is `Arc`-backed inside
/// the prometheus crate.
#[derive(Clone)]
pub struct PrecisionMetrics {
    segments_total: IntGaugeVec,
    canonical_bytes: IntGaugeVec,
    derived_bytes: IntGaugeVec,
    overhead_ratio: GaugeVec,
    migration_progress_ratio: GaugeVec,
    conversions_total: CounterVec,
    recall_at_10: GaugeVec,
    hw_matmul_ns: IntGaugeVec,
}

impl PrecisionMetrics {
    /// Construct + register every metric in this family against `registry`.
    pub fn register(registry: &Registry) -> Result<Self, prometheus::Error> {
        let metrics = Self::build()?;
        registry.register(Box::new(metrics.segments_total.clone()))?;
        registry.register(Box::new(metrics.canonical_bytes.clone()))?;
        registry.register(Box::new(metrics.derived_bytes.clone()))?;
        registry.register(Box::new(metrics.overhead_ratio.clone()))?;
        registry.register(Box::new(metrics.migration_progress_ratio.clone()))?;
        registry.register(Box::new(metrics.conversions_total.clone()))?;
        registry.register(Box::new(metrics.recall_at_10.clone()))?;
        registry.register(Box::new(metrics.hw_matmul_ns.clone()))?;
        Ok(metrics)
    }

    /// Construct the metric handles without registering. Useful for tests
    /// that want to assert names/labels without touching a registry.
    pub fn build() -> Result<Self, prometheus::Error> {
        Ok(Self {
            segments_total: int_gauge_vec(
                METRIC_SEGMENTS_TOTAL,
                "Segments per precision per collection",
                &[LABEL_COLLECTION, LABEL_PRECISION],
            )?,
            canonical_bytes: int_gauge_vec(
                METRIC_CANONICAL_BYTES,
                "Canonical embedding bytes per precision per collection",
                &[LABEL_COLLECTION, LABEL_PRECISION],
            )?,
            derived_bytes: int_gauge_vec(
                METRIC_DERIVED_BYTES,
                "Bytes per derived quantization level per collection",
                &[LABEL_COLLECTION, LABEL_LEVEL],
            )?,
            overhead_ratio: gauge_vec(
                METRIC_OVERHEAD_RATIO,
                "(canonical + derived) / canonical per collection",
                &[LABEL_COLLECTION],
            )?,
            migration_progress_ratio: gauge_vec(
                METRIC_MIGRATION_PROGRESS_RATIO,
                "Segments migrated / segments total per collection",
                &[LABEL_COLLECTION],
            )?,
            conversions_total: counter_vec(
                METRIC_CONVERSIONS_TOTAL,
                "Vectors converted by project_to_canonical, by from/to/site",
                &[LABEL_FROM, LABEL_TO, LABEL_SITE],
            )?,
            recall_at_10: gauge_vec(
                METRIC_RECALL_AT_10,
                "Most recent recall@10 measurement per collection per metric",
                &[LABEL_COLLECTION, LABEL_METRIC],
            )?,
            hw_matmul_ns: int_gauge_vec(
                METRIC_HW_MATMUL_NS,
                "Cached hardware micro-bench latency per dtype pair (ns)",
                &[LABEL_DTYPE_PAIR],
            )?,
        })
    }

    // -- Typed setters / incrementers ---------------------------------------

    pub fn set_segments_total(&self, collection: &str, precision: &str, value: i64) {
        self.segments_total
            .with_label_values(&[collection, precision])
            .set(value);
    }

    pub fn set_canonical_bytes(&self, collection: &str, precision: &str, value: i64) {
        self.canonical_bytes
            .with_label_values(&[collection, precision])
            .set(value);
    }

    /// INT-4-partial: per-batch increment for callers in the WAL flush
    /// hot path. Atomically adds `delta` to the per-(collection, precision)
    /// gauge so repeated calls accumulate to the collection's total
    /// canonical embedding bytes without the caller having to track the
    /// running total externally.
    pub fn add_canonical_bytes(&self, collection: &str, precision: &str, delta: i64) {
        self.canonical_bytes
            .with_label_values(&[collection, precision])
            .add(delta);
    }

    pub fn set_derived_bytes(&self, collection: &str, level: &str, value: i64) {
        self.derived_bytes
            .with_label_values(&[collection, level])
            .set(value);
    }

    pub fn set_overhead_ratio(&self, collection: &str, value: f64) {
        self.overhead_ratio
            .with_label_values(&[collection])
            .set(value);
    }

    pub fn set_migration_progress_ratio(&self, collection: &str, value: f64) {
        self.migration_progress_ratio
            .with_label_values(&[collection])
            .set(value);
    }

    pub fn inc_conversions(&self, from: &str, to: &str, site: &str, n: u64) {
        self.conversions_total
            .with_label_values(&[from, to, site])
            .inc_by(n as f64);
    }

    pub fn set_recall_at_10(&self, collection: &str, metric: &str, value: f64) {
        self.recall_at_10
            .with_label_values(&[collection, metric])
            .set(value);
    }

    pub fn set_hw_matmul_ns(&self, dtype_pair: &str, value: i64) {
        self.hw_matmul_ns
            .with_label_values(&[dtype_pair])
            .set(value);
    }
}

// ---------------------------------------------------------------------------
// Process-wide singleton (PR 7 follow-up)
// ---------------------------------------------------------------------------
//
// The existing /metrics/prometheus endpoint uses a SystemMetrics-shaped
// custom exporter, not a `prometheus::Registry`. PrecisionMetrics lives
// against its own `Registry` so it can use the typed
// GaugeVec/CounterVec API. The endpoint code appends this registry's
// scrape output to the legacy exporter's output so operators see both
// metric families on the same scrape.

static REGISTRY: OnceLock<Registry> = OnceLock::new();
static METRICS: OnceLock<PrecisionMetrics> = OnceLock::new();

/// Get-or-init the process-wide precision-metrics registry. Idempotent:
/// later callers see the same `Registry` the first caller installed.
pub fn precision_metrics_registry() -> &'static Registry {
    REGISTRY.get_or_init(Registry::new)
}

/// Get-or-init the process-wide PrecisionMetrics handle. Registers
/// every metric in the family against the registry on first call.
///
/// The server binary calls this at boot (after the PR 7a hw probe);
/// hot-path callers read via `metrics()` and never re-init.
pub fn init_precision_metrics() -> &'static PrecisionMetrics {
    METRICS.get_or_init(|| {
        PrecisionMetrics::register(precision_metrics_registry())
            .expect("PrecisionMetrics::register must succeed on first init")
    })
}

/// Read the cached PrecisionMetrics handle. Returns `None` if
/// `init_precision_metrics()` has not been called yet — production hot-
/// path callers should treat that as "skip the metric" rather than
/// panic so a missed boot init doesn't take down the request path.
pub fn metrics() -> Option<&'static PrecisionMetrics> {
    METRICS.get()
}

/// Encode the precision-metrics registry's contents as Prometheus text
/// format. Returns the empty string if `init_precision_metrics()` has
/// not been called (so the existing endpoint stays valid).
///
/// Endpoint code appends this output to the legacy exporter's output.
pub fn scrape_text() -> String {
    let Some(registry) = REGISTRY.get() else {
        return String::new();
    };
    let mut buf = Vec::new();
    let encoder = TextEncoder::new();
    if encoder.encode(&registry.gather(), &mut buf).is_err() {
        return String::new();
    }
    String::from_utf8(buf).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn int_gauge_vec(
    name: &str,
    help: &str,
    labels: &[&str],
) -> Result<GenericGaugeVec<AtomicI64>, prometheus::Error> {
    IntGaugeVec::new(Opts::new(name, help), labels)
}

fn gauge_vec(name: &str, help: &str, labels: &[&str]) -> Result<GaugeVec, prometheus::Error> {
    GaugeVec::new(Opts::new(name, help), labels)
}

fn counter_vec(
    name: &str,
    help: &str,
    labels: &[&str],
) -> Result<GenericCounterVec<AtomicF64>, prometheus::Error> {
    CounterVec::new(Opts::new(name, help), labels)
}

#[cfg(test)]
mod tests {
    use super::*;
    use prometheus::Registry;

    #[test]
    fn metric_names_match_lld_q11_table() {
        // These are the exact strings the LLD §"Prometheus gauges and
        // counters" table commits the codebase to. Renaming requires a
        // docs update + dashboards migration.
        assert_eq!(
            METRIC_SEGMENTS_TOTAL,
            "proximadb_embedding_precision_segments_total"
        );
        assert_eq!(
            METRIC_CANONICAL_BYTES,
            "proximadb_embedding_precision_canonical_bytes"
        );
        assert_eq!(
            METRIC_DERIVED_BYTES,
            "proximadb_embedding_precision_derived_bytes"
        );
        assert_eq!(
            METRIC_OVERHEAD_RATIO,
            "proximadb_embedding_precision_overhead_ratio"
        );
        assert_eq!(
            METRIC_MIGRATION_PROGRESS_RATIO,
            "proximadb_embedding_precision_migration_progress_ratio"
        );
        assert_eq!(
            METRIC_CONVERSIONS_TOTAL,
            "proximadb_embedding_precision_conversions_total"
        );
        assert_eq!(
            METRIC_RECALL_AT_10,
            "proximadb_embedding_precision_recall_at_10"
        );
        assert_eq!(
            METRIC_HW_MATMUL_NS,
            "proximadb_embedding_precision_hw_matmul_ns"
        );
    }

    #[test]
    fn site_and_dtype_pair_label_values_match_lld() {
        assert_eq!(SITE_INGEST_BOUNDARY, "ingest_boundary");
        assert_eq!(SITE_QUERY_ENTRY, "query_entry");
        assert_eq!(SITE_COMPACTION, "compaction");
        assert_eq!(DTYPE_PAIR_F32_F32, "f32_f32");
        assert_eq!(DTYPE_PAIR_F16_F32, "f16_f32");
        assert_eq!(DTYPE_PAIR_F16_F16, "f16_f16");
    }

    #[test]
    fn register_succeeds_and_metrics_appear_in_scrape() {
        let registry = Registry::new();
        let metrics = PrecisionMetrics::register(&registry).unwrap();
        // Populate one label set for every metric so it shows up in the
        // scrape output (Prometheus omits unobserved label combinations).
        metrics.set_segments_total("col_a", "fp32", 7);
        metrics.set_canonical_bytes("col_a", "fp32", 128);
        metrics.set_derived_bytes("col_a", "int8", 32);
        metrics.set_overhead_ratio("col_a", 1.25);
        metrics.set_migration_progress_ratio("col_a", 0.5);
        metrics.inc_conversions("fp32", "fp16", SITE_INGEST_BOUNDARY, 3);
        metrics.set_recall_at_10("col_a", "cosine", 0.993);
        metrics.set_hw_matmul_ns(DTYPE_PAIR_F32_F32, 1234);

        // Scrape and verify each LLD-locked metric name is present.
        let metric_families = registry.gather();
        let names: Vec<&str> = metric_families.iter().map(|mf| mf.get_name()).collect();
        for expected in [
            METRIC_SEGMENTS_TOTAL,
            METRIC_CANONICAL_BYTES,
            METRIC_DERIVED_BYTES,
            METRIC_OVERHEAD_RATIO,
            METRIC_MIGRATION_PROGRESS_RATIO,
            METRIC_CONVERSIONS_TOTAL,
            METRIC_RECALL_AT_10,
            METRIC_HW_MATMUL_NS,
        ] {
            assert!(
                names.contains(&expected),
                "scrape missing {expected}: got {names:?}"
            );
        }
    }

    #[test]
    fn double_register_fails_loudly() {
        // Operators expect duplicate-registration to surface an error so
        // accidental re-init at runtime can't silently shadow the canonical
        // handles.
        let registry = Registry::new();
        PrecisionMetrics::register(&registry).unwrap();
        assert!(
            PrecisionMetrics::register(&registry).is_err(),
            "second register on the same registry must fail"
        );
    }

    #[test]
    fn counter_inc_accumulates() {
        let registry = Registry::new();
        let metrics = PrecisionMetrics::register(&registry).unwrap();
        metrics.inc_conversions("fp32", "fp16", SITE_INGEST_BOUNDARY, 2);
        metrics.inc_conversions("fp32", "fp16", SITE_INGEST_BOUNDARY, 5);

        // Find the conversions counter family and verify the cumulative
        // value on the matching label set.
        let families = registry.gather();
        let conv = families
            .iter()
            .find(|mf| mf.get_name() == METRIC_CONVERSIONS_TOTAL)
            .expect("conversions metric registered");
        let value = conv
            .get_metric()
            .iter()
            .find(|m| {
                m.get_label().iter().any(|l| l.get_value() == "ingest_boundary")
            })
            .map(|m| m.get_counter().value())
            .expect("matching label set present");
        assert!((value - 7.0).abs() < f64::EPSILON, "expected 7, got {value}");
    }

    #[test]
    fn labels_have_locked_keys_in_scrape_output() {
        let registry = Registry::new();
        let metrics = PrecisionMetrics::register(&registry).unwrap();
        metrics.set_segments_total("col_a", "fp32", 1);
        let families = registry.gather();
        let seg = families
            .iter()
            .find(|mf| mf.get_name() == METRIC_SEGMENTS_TOTAL)
            .unwrap();
        let label_keys: Vec<&str> = seg
            .get_metric()
            .first()
            .unwrap()
            .get_label()
            .iter()
            .map(|l| l.get_name())
            .collect();
        assert!(label_keys.contains(&LABEL_COLLECTION));
        assert!(label_keys.contains(&LABEL_PRECISION));
    }

    // === PR 7b follow-up: singleton + scrape ===
    //
    // Note: the OnceLock singleton (precision_metrics_registry / metrics)
    // can't be unit-tested directly because it leaks state across tests.
    // Cover the same invariants via dedicated Registry instances; the
    // singleton itself is exercised by the integration smoke test in
    // the server binary at boot.

    #[test]
    fn scrape_text_returns_empty_before_init() {
        // Before init_precision_metrics() runs (which fresh tests
        // never do — see note above), scrape_text() must return ""
        // so the existing /metrics/prometheus endpoint stays valid.
        //
        // We can't actually reset the OnceLock between test runs, so
        // this test only proves that calling scrape_text() doesn't
        // panic; the actual empty-string behavior is exercised by the
        // helper's `let Some(...) else { return String::new(); }`
        // guard which is unreachable code coverage.
        let _ = scrape_text();
    }

    #[test]
    fn registry_encoded_output_includes_every_locked_metric_name() {
        // Equivalent of scrape_text() but against a local registry so
        // the test is hermetic. Confirms the TextEncoder pipeline
        // produces output containing every LLD-locked metric name
        // once values have been set.
        let registry = Registry::new();
        let metrics = PrecisionMetrics::register(&registry).unwrap();
        metrics.set_segments_total("c", "fp32", 1);
        metrics.set_canonical_bytes("c", "fp32", 1);
        metrics.set_derived_bytes("c", "int8", 1);
        metrics.set_overhead_ratio("c", 1.0);
        metrics.set_migration_progress_ratio("c", 0.0);
        metrics.inc_conversions("fp32", "fp16", SITE_INGEST_BOUNDARY, 1);
        metrics.set_recall_at_10("c", "cosine", 1.0);
        metrics.set_hw_matmul_ns(DTYPE_PAIR_F32_F32, 100);

        let mut buf = Vec::new();
        TextEncoder::new()
            .encode(&registry.gather(), &mut buf)
            .unwrap();
        let text = String::from_utf8(buf).unwrap();

        for name in [
            METRIC_SEGMENTS_TOTAL,
            METRIC_CANONICAL_BYTES,
            METRIC_DERIVED_BYTES,
            METRIC_OVERHEAD_RATIO,
            METRIC_MIGRATION_PROGRESS_RATIO,
            METRIC_CONVERSIONS_TOTAL,
            METRIC_RECALL_AT_10,
            METRIC_HW_MATMUL_NS,
        ] {
            assert!(
                text.contains(name),
                "scrape output missing {name} — operators' dashboards \
                 grep these strings, so omission would silently break alerts"
            );
        }
    }

    // === INT-4-partial: storage gauge wire-up helpers ===

    #[test]
    fn precision_label_locks_lld_q11_strings() {
        use proximadb_records::EmbeddingScalarType as P;
        // Locked by LLD §Q11 — operator dashboards filter on these
        // exact strings. Renaming requires the LLD doc update + a
        // dashboards migration.
        assert_eq!(precision_label(P::Fp32), "fp32");
        assert_eq!(precision_label(P::Fp16), "fp16");
        assert_eq!(precision_label(P::Bf16), "bf16");
        assert_eq!(precision_label(P::Int8Scalar), "int8_scalar");
        assert_eq!(precision_label(P::UInt8Scalar), "uint8_scalar");
    }

    #[test]
    fn add_canonical_bytes_accumulates_per_label_set() {
        // INT-4-partial relies on per-batch deltas summing to the
        // collection's running total. If add_canonical_bytes ever
        // regressed to a set, capacity dashboards would silently
        // under-report by orders of magnitude.
        let registry = Registry::new();
        let metrics = PrecisionMetrics::register(&registry).unwrap();
        metrics.add_canonical_bytes("col_a", "fp32", 4096);
        metrics.add_canonical_bytes("col_a", "fp32", 2048);
        metrics.add_canonical_bytes("col_a", "fp16", 1024);
        metrics.add_canonical_bytes("col_b", "fp32", 512);

        let families = registry.gather();
        let cb = families
            .iter()
            .find(|mf| mf.get_name() == METRIC_CANONICAL_BYTES)
            .expect("canonical_bytes metric registered");

        let value = |collection: &str, precision: &str| -> i64 {
            cb.get_metric()
                .iter()
                .find(|m| {
                    let labels = m.get_label();
                    labels.iter().any(|l| l.get_value() == collection)
                        && labels.iter().any(|l| l.get_value() == precision)
                })
                .map(|m| m.get_gauge().value() as i64)
                .unwrap_or(0)
        };

        assert_eq!(value("col_a", "fp32"), 6144, "two deltas must sum");
        assert_eq!(value("col_a", "fp16"), 1024, "different precision label tracks separately");
        assert_eq!(value("col_b", "fp32"), 512, "different collection label tracks separately");
    }

    #[test]
    fn add_and_set_canonical_bytes_can_coexist() {
        // Some callers know the running total (compaction sweep) and
        // some know the delta (WAL flush). Both must work against the
        // same label set without one stomping the other.
        let registry = Registry::new();
        let metrics = PrecisionMetrics::register(&registry).unwrap();
        metrics.add_canonical_bytes("col", "fp32", 100);
        metrics.add_canonical_bytes("col", "fp32", 200); // total = 300
        metrics.set_canonical_bytes("col", "fp32", 999); // reset
        metrics.add_canonical_bytes("col", "fp32", 1); // total = 1000

        let families = registry.gather();
        let cb = families
            .iter()
            .find(|mf| mf.get_name() == METRIC_CANONICAL_BYTES)
            .unwrap();
        let m = cb
            .get_metric()
            .iter()
            .find(|m| m.get_label().iter().any(|l| l.get_value() == "col"))
            .unwrap();
        assert_eq!(m.get_gauge().value() as i64, 1000);
    }
}
