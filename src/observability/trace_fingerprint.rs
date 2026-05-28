// Trace fingerprint — shape-only hash, the read-side dual of trace_digest.
//
// `trace_digest::digest` hashes trace **identity** (tenant_id, trace_id,
// occurred_at_bucket) for dedup. This module hashes trace **shape**:
// the structural decisions the planner + runtime made, *excluding*
// identity. Two traces from different tenants on different days that
// took the same plan and hit the same kind of trouble share a
// fingerprint.
//
// The use case is incident triage. An SRE sees a spike of failing
// queries and runs "group by fingerprint" against the trace store —
// the result is a small set of buckets like:
//
//   fp=fbe9...  PostFilter / FullPrecisionGraph / miss / scan>50% / 4 traces
//   fp=8a40...  HybridFilter / Quantized / hit / scan<10% / 1290 traces
//
// without having to eyeball 30 fields per trace.
//
// Shape inputs:
//   - filter_strategy
//   - index_route
//   - cache_result
//   - failure_class (None = no failure)
//   - scan_band (4 buckets: <10%, <30%, <60%, ≥60%)
//   - latency_band (4 buckets: <50ms, <250ms, <1s, ≥1s)
//   - repair_count_bucket (0, 1, 2+)
//   - quantized_route_taken (bool)
//
// Identity fields (tenant_id, trace_id, occurred_at, collection_name)
// are deliberately excluded. A future opt-in `with_collection()`
// variant could include collection_name when the SRE wants per-collection
// grouping — left out of v1 to keep the bucket count manageable.

use crate::observability::search_plan_trace::{
    CacheResult, FailureClass, FilterStrategy, IndexRoute, SearchPlanTrace,
};

/// Bounded scan-fraction band. Pin the boundaries so a dashboard's
/// legend can show what each label means without re-deriving.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ScanBand {
    Tiny,   // <10%
    Small,  // <30%
    Medium, // <60%
    Large,  // ≥60%
}

impl ScanBand {
    pub fn from_fraction(f: f64) -> Self {
        if !f.is_finite() || f <= 0.0 {
            return ScanBand::Tiny;
        }
        let f = f.clamp(0.0, 1.0);
        if f < 0.10 {
            ScanBand::Tiny
        } else if f < 0.30 {
            ScanBand::Small
        } else if f < 0.60 {
            ScanBand::Medium
        } else {
            ScanBand::Large
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            ScanBand::Tiny => "tiny",
            ScanBand::Small => "small",
            ScanBand::Medium => "medium",
            ScanBand::Large => "large",
        }
    }
}

/// Bounded latency band. Boundaries match common SLA tiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LatencyBand {
    Fast,    // <50ms
    Normal,  // <250ms
    Slow,    // <1s
    Stalled, // ≥1s
}

impl LatencyBand {
    pub fn from_ms(ms: f64) -> Self {
        if !ms.is_finite() || ms < 0.0 {
            return LatencyBand::Fast;
        }
        if ms < 50.0 {
            LatencyBand::Fast
        } else if ms < 250.0 {
            LatencyBand::Normal
        } else if ms < 1000.0 {
            LatencyBand::Slow
        } else {
            LatencyBand::Stalled
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            LatencyBand::Fast => "fast",
            LatencyBand::Normal => "normal",
            LatencyBand::Slow => "slow",
            LatencyBand::Stalled => "stalled",
        }
    }
}

/// Bounded repair-count bucket. The LLD §9 contract caps repair at one
/// pass; 2+ is "controller decided multiple repair passes" which only
/// happens if the cap is overridden in config.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RepairBucket {
    None,
    Single,
    Multiple,
}

impl RepairBucket {
    pub fn from_count(n: u32) -> Self {
        match n {
            0 => RepairBucket::None,
            1 => RepairBucket::Single,
            _ => RepairBucket::Multiple,
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            RepairBucket::None => "none",
            RepairBucket::Single => "single",
            RepairBucket::Multiple => "multiple",
        }
    }
}

/// Shape vector — what the fingerprint hashes. Public so callers can
/// emit it alongside the fingerprint hex for dashboard legends.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TraceShape {
    pub filter_strategy: FilterStrategy,
    pub index_route: IndexRoute,
    pub cache_result: CacheResult,
    pub failure_class: Option<FailureClass>,
    pub scan_band: ScanBand,
    pub latency_band: LatencyBand,
    pub repair_bucket: RepairBucket,
    pub quantized_route_taken: bool,
}

impl TraceShape {
    pub fn from_trace(trace: &SearchPlanTrace, corpus_gb: f64) -> Self {
        let scan_fraction = if corpus_gb > 0.0 && corpus_gb.is_finite() {
            (trace.actual_scan_gb / corpus_gb).clamp(0.0, 1.0)
        } else {
            0.0
        };
        Self {
            filter_strategy: trace.filter_strategy.clone(),
            index_route: trace.index_route.clone(),
            cache_result: trace.cache_result.clone(),
            failure_class: trace.failure_class.clone(),
            scan_band: ScanBand::from_fraction(scan_fraction),
            latency_band: LatencyBand::from_ms(trace.latency_ms),
            repair_bucket: RepairBucket::from_count(trace.repair_count),
            quantized_route_taken: matches!(trace.index_route, IndexRoute::QuantizedGraphThenExact),
        }
    }

    /// Compose a stable, hashable byte stream covering every shape
    /// field. Used internally by `fingerprint`; exposed for tests that
    /// want to assert the encoding contract.
    pub fn encode_for_hash(&self) -> Vec<u8> {
        let mut bytes: Vec<u8> = Vec::with_capacity(64);
        push_label(&mut bytes, filter_strategy_label(&self.filter_strategy));
        push_label(&mut bytes, index_route_label(&self.index_route));
        push_label(&mut bytes, cache_result_label(&self.cache_result));
        push_label(
            &mut bytes,
            self.failure_class
                .as_ref()
                .map(failure_class_label)
                .unwrap_or("none"),
        );
        push_label(&mut bytes, self.scan_band.label());
        push_label(&mut bytes, self.latency_band.label());
        push_label(&mut bytes, self.repair_bucket.label());
        bytes.push(self.quantized_route_taken as u8);
        bytes
    }
}

fn push_label(bytes: &mut Vec<u8>, s: &str) {
    bytes.extend_from_slice(s.as_bytes());
    bytes.push(0x1f); // ASCII unit separator
}

/// Compute the 64-bit shape fingerprint. FNV-1a over the encoded shape.
pub fn fingerprint(shape: &TraceShape) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in shape.encode_for_hash() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

/// Hex-encode the fingerprint as a 16-char lowercase string. Useful as
/// the `fingerprint` field on the trace's metadata block.
pub fn fingerprint_hex(shape: &TraceShape) -> String {
    format!("{:016x}", fingerprint(shape))
}

fn filter_strategy_label(s: &FilterStrategy) -> &'static str {
    match s {
        FilterStrategy::PreFilter => "pre_filter",
        FilterStrategy::HybridFilter => "hybrid_filter",
        FilterStrategy::PostFilter => "post_filter",
    }
}

fn index_route_label(r: &IndexRoute) -> &'static str {
    match r {
        IndexRoute::QuantizedGraphThenExact => "quantized",
        IndexRoute::FullPrecisionGraph => "full_precision",
        IndexRoute::LexicalThenVector => "lexical_then_vector",
        IndexRoute::VectorThenLexical => "vector_then_lexical",
        IndexRoute::GraphWalk => "graph_walk",
    }
}

fn cache_result_label(c: &CacheResult) -> &'static str {
    match c {
        CacheResult::Hit => "hit",
        CacheResult::Miss => "miss",
        CacheResult::FalseHit => "false_hit",
        CacheResult::Bypass => "bypass",
    }
}

fn failure_class_label(f: &FailureClass) -> &'static str {
    match f {
        FailureClass::BudgetExhausted => "budget_exhausted",
        FailureClass::LowCoverage => "low_coverage",
        FailureClass::Contradiction => "contradiction",
        FailureClass::StaleEvidence => "stale_evidence",
        FailureClass::OverBroadRetrieval => "over_broad_retrieval",
        FailureClass::PermissionThin => "permission_thin",
        FailureClass::InsufficientEvidence => "insufficient_evidence",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::service_types::IndexStats;
    use crate::observability::search_plan_trace::SureSignals;

    fn trace_template() -> SearchPlanTrace {
        SearchPlanTrace {
            trace_id: "t1".into(),
            tenant_id: "tenant-a".into(),
            collection_name: "kb".into(),
            plan_version: 1,
            filter_strategy: FilterStrategy::HybridFilter,
            index_route: IndexRoute::FullPrecisionGraph,
            cache_result: CacheResult::Miss,
            estimated_selectivity: None,
            actual_selectivity: None,
            gls_score: None,
            estimated_scan_gb: None,
            actual_scan_gb: 0.0,
            index_stats: IndexStats::default(),
            candidate_count: 0,
            rerank_count: 0,
            repair_count: 0,
            sure_signals: SureSignals::default(),
            latency_ms: 12.3,
            recall_probe_score: None,
            utility_score_avg: None,
            failure_class: None,
            predicate_shortfall: None,
        }
    }

    #[test]
    fn scan_band_thresholds_pin_to_doc_strings() {
        assert_eq!(ScanBand::from_fraction(0.0), ScanBand::Tiny);
        assert_eq!(ScanBand::from_fraction(0.05), ScanBand::Tiny);
        assert_eq!(ScanBand::from_fraction(0.10), ScanBand::Small);
        assert_eq!(ScanBand::from_fraction(0.29), ScanBand::Small);
        assert_eq!(ScanBand::from_fraction(0.30), ScanBand::Medium);
        assert_eq!(ScanBand::from_fraction(0.59), ScanBand::Medium);
        assert_eq!(ScanBand::from_fraction(0.60), ScanBand::Large);
        assert_eq!(ScanBand::from_fraction(1.0), ScanBand::Large);
    }

    #[test]
    fn scan_band_negative_and_nan_collapse_to_tiny() {
        assert_eq!(ScanBand::from_fraction(-0.5), ScanBand::Tiny);
        assert_eq!(ScanBand::from_fraction(f64::NAN), ScanBand::Tiny);
    }

    #[test]
    fn scan_band_labels_are_bounded() {
        let labels = [
            ScanBand::Tiny.label(),
            ScanBand::Small.label(),
            ScanBand::Medium.label(),
            ScanBand::Large.label(),
        ];
        assert_eq!(labels, ["tiny", "small", "medium", "large"]);
    }

    #[test]
    fn latency_band_thresholds() {
        assert_eq!(LatencyBand::from_ms(0.0), LatencyBand::Fast);
        assert_eq!(LatencyBand::from_ms(49.0), LatencyBand::Fast);
        assert_eq!(LatencyBand::from_ms(50.0), LatencyBand::Normal);
        assert_eq!(LatencyBand::from_ms(249.0), LatencyBand::Normal);
        assert_eq!(LatencyBand::from_ms(250.0), LatencyBand::Slow);
        assert_eq!(LatencyBand::from_ms(999.0), LatencyBand::Slow);
        assert_eq!(LatencyBand::from_ms(1000.0), LatencyBand::Stalled);
        assert_eq!(LatencyBand::from_ms(60_000.0), LatencyBand::Stalled);
    }

    #[test]
    fn latency_band_negative_and_nan_collapse_to_fast() {
        assert_eq!(LatencyBand::from_ms(-1.0), LatencyBand::Fast);
        assert_eq!(LatencyBand::from_ms(f64::NAN), LatencyBand::Fast);
    }

    #[test]
    fn repair_bucket_caps_at_multiple() {
        assert_eq!(RepairBucket::from_count(0), RepairBucket::None);
        assert_eq!(RepairBucket::from_count(1), RepairBucket::Single);
        assert_eq!(RepairBucket::from_count(2), RepairBucket::Multiple);
        assert_eq!(RepairBucket::from_count(50), RepairBucket::Multiple);
    }

    #[test]
    fn shape_excludes_identity_fields() {
        // Two traces with different tenant_id + trace_id + collection
        // + occurred_at but identical shape → identical fingerprint.
        let a = trace_template();
        let mut b = trace_template();
        b.trace_id = "t2".into();
        b.tenant_id = "tenant-b".into();
        b.collection_name = "kb-other".into();
        let sa = TraceShape::from_trace(&a, 1.0);
        let sb = TraceShape::from_trace(&b, 1.0);
        assert_eq!(sa, sb);
        assert_eq!(fingerprint(&sa), fingerprint(&sb));
    }

    #[test]
    fn distinct_filter_strategy_yields_distinct_fingerprint() {
        let a = trace_template();
        let mut b = trace_template();
        b.filter_strategy = FilterStrategy::PreFilter;
        let sa = TraceShape::from_trace(&a, 1.0);
        let sb = TraceShape::from_trace(&b, 1.0);
        assert_ne!(fingerprint(&sa), fingerprint(&sb));
    }

    #[test]
    fn distinct_index_route_yields_distinct_fingerprint() {
        let a = trace_template();
        let mut b = trace_template();
        b.index_route = IndexRoute::QuantizedGraphThenExact;
        let sa = TraceShape::from_trace(&a, 1.0);
        let sb = TraceShape::from_trace(&b, 1.0);
        assert_ne!(fingerprint(&sa), fingerprint(&sb));
        // And the quantized flag is set.
        assert!(sb.quantized_route_taken);
    }

    #[test]
    fn scan_band_groups_within_band() {
        // 5% and 8% both fall in Tiny → same fingerprint.
        let mut a = trace_template();
        let mut b = trace_template();
        a.actual_scan_gb = 0.05;
        b.actual_scan_gb = 0.08;
        let sa = TraceShape::from_trace(&a, 1.0);
        let sb = TraceShape::from_trace(&b, 1.0);
        assert_eq!(fingerprint(&sa), fingerprint(&sb));
    }

    #[test]
    fn scan_band_splits_across_bands() {
        // 5% (Tiny) vs 15% (Small) → distinct fingerprint.
        let mut a = trace_template();
        let mut b = trace_template();
        a.actual_scan_gb = 0.05;
        b.actual_scan_gb = 0.15;
        let sa = TraceShape::from_trace(&a, 1.0);
        let sb = TraceShape::from_trace(&b, 1.0);
        assert_ne!(fingerprint(&sa), fingerprint(&sb));
    }

    #[test]
    fn latency_band_groups_within_band() {
        let mut a = trace_template();
        let mut b = trace_template();
        a.latency_ms = 12.0;
        b.latency_ms = 30.0;
        let sa = TraceShape::from_trace(&a, 1.0);
        let sb = TraceShape::from_trace(&b, 1.0);
        assert_eq!(fingerprint(&sa), fingerprint(&sb), "both Fast band");
    }

    #[test]
    fn failure_class_changes_fingerprint() {
        let a = trace_template();
        let mut b = trace_template();
        b.failure_class = Some(FailureClass::BudgetExhausted);
        let sa = TraceShape::from_trace(&a, 1.0);
        let sb = TraceShape::from_trace(&b, 1.0);
        assert_ne!(fingerprint(&sa), fingerprint(&sb));
    }

    #[test]
    fn cache_result_changes_fingerprint() {
        let a = trace_template();
        let mut b = trace_template();
        b.cache_result = CacheResult::Hit;
        let sa = TraceShape::from_trace(&a, 1.0);
        let sb = TraceShape::from_trace(&b, 1.0);
        assert_ne!(fingerprint(&sa), fingerprint(&sb));
    }

    #[test]
    fn hex_is_16_chars_lowercase() {
        let t = trace_template();
        let s = TraceShape::from_trace(&t, 1.0);
        let h = fingerprint_hex(&s);
        assert_eq!(h.len(), 16);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
    }

    #[test]
    fn fingerprint_round_trips_via_hex() {
        let t = trace_template();
        let s = TraceShape::from_trace(&t, 1.0);
        let h = fingerprint_hex(&s);
        let back = u64::from_str_radix(&h, 16).unwrap();
        assert_eq!(fingerprint(&s), back);
    }

    #[test]
    fn encoding_uses_field_delimiter() {
        // The unit-separator byte (0x1f) appears between labels so a
        // single-char shift of label boundaries doesn't accidentally
        // produce the same byte stream.
        let t = trace_template();
        let s = TraceShape::from_trace(&t, 1.0);
        let bytes = s.encode_for_hash();
        assert!(
            bytes.contains(&0x1f),
            "encoding must include unit separator"
        );
    }

    #[test]
    fn corpus_gb_zero_treats_scan_fraction_as_zero() {
        // Without a corpus size we can't compute scan fraction; treat
        // it as zero rather than guessing.
        let mut t = trace_template();
        t.actual_scan_gb = 0.5;
        let s = TraceShape::from_trace(&t, 0.0);
        assert_eq!(s.scan_band, ScanBand::Tiny);
    }

    #[test]
    fn shape_is_hash_eq() {
        // TraceShape derives Hash + Eq so it works as a HashMap key
        // for the gateway's per-fingerprint counters.
        let t = trace_template();
        let s = TraceShape::from_trace(&t, 1.0);
        let mut map = std::collections::HashMap::new();
        map.insert(s.clone(), 1u32);
        *map.entry(s).or_insert(0) += 1;
        assert_eq!(map.values().sum::<u32>(), 2);
    }

    #[test]
    fn quantized_route_taken_flag_matches_index_route() {
        let mut t = trace_template();
        t.index_route = IndexRoute::QuantizedGraphThenExact;
        let s = TraceShape::from_trace(&t, 1.0);
        assert!(s.quantized_route_taken);

        let mut t2 = trace_template();
        t2.index_route = IndexRoute::GraphWalk;
        let s2 = TraceShape::from_trace(&t2, 1.0);
        assert!(!s2.quantized_route_taken);
    }
}
