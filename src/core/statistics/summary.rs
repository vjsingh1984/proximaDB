// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Resident per-collection statistics summary (ADR-037 TD-174).
//!
//! A bounded, mutable accumulator maintained at the flush/compaction write
//! boundary — the sibling of the ADR-030 KSU resident-bytes meter. It folds
//! per-record observations and PAX zone-map bounds into mergeable streaming
//! sketches, then projects the frozen v1 [`StatisticsEnvelope`] on demand. The
//! engine never rescans the corpus: an envelope is always available from this
//! tiny resident summary (Decision 1).
//!
//! The summary lives in the main crate (it references [`super`]'s envelope
//! types); the storage crate that owns the PAX writer cannot depend on it, so
//! the flush hook in the main crate drives [`StatisticsSummary::observe_field`]
//! / [`StatisticsSummary::merge_zone_bounds`] from records/zone-maps surfaced at
//! the write boundary.

use super::sketches::{FrequentItems, HyperLogLog, Reservoir, TDigest};
use super::{
    DocumentStatistics, ENVELOPE_VERSION, FieldStatistics, Freshness, GraphStatistics,
    ModalityStatistics, Quantiles, StatisticsEnvelope, TermStat, TimeseriesStatistics,
    TraceStatistics, VectorStatistics,
};
use serde_json::Value;
use std::collections::BTreeMap;

/// How many example values / frequent items / quantiles to retain per field.
const RESERVOIR_CAP: usize = 8;
const TOP_TERMS_CAP: usize = 16;
/// Quantiles emitted from the t-digest (matches the golden example shape).
const EMITTED_QUANTILES: [f64; 3] = [0.5, 0.9, 0.99];

/// Per-field distribution accumulator.
#[derive(Debug, Clone)]
pub struct FieldAccumulator {
    name: String,
    data_type: Value,
    numeric: bool,
    total: u64,
    nulls: u64,
    distinct: HyperLogLog,
    quantiles: TDigest,
    examples: Reservoir,
    /// Running typed min/max. Maintained from observed values AND merged
    /// zone-map bounds (so it is correct even when only block bounds are known).
    min: Option<Value>,
    max: Option<Value>,
    indexed: Option<bool>,
    filterable: Option<bool>,
    /// Any sketch-derived statistic was used → labeled approximate in the wire.
    approximate: bool,
}

impl FieldAccumulator {
    fn new(name: String, data_type: Value) -> Self {
        let numeric = is_numeric_type(&data_type);
        Self {
            name,
            data_type,
            numeric,
            total: 0,
            nulls: 0,
            distinct: HyperLogLog::new(),
            quantiles: TDigest::default(),
            examples: Reservoir::new(RESERVOIR_CAP),
            min: None,
            max: None,
            indexed: None,
            filterable: None,
            approximate: false,
        }
    }

    fn observe(&mut self, value: Option<&Value>) {
        self.total += 1;
        let v = match value {
            Some(v) if !v.is_null() => v,
            _ => {
                self.nulls += 1;
                return;
            }
        };
        // Distinct + examples (every type).
        let canon = canonical_string(v);
        self.distinct.insert_bytes(canon.as_bytes());
        self.examples.insert(&canon);
        self.approximate = true;
        // Quantiles for numeric fields.
        if self.numeric
            && let Some(f) = v.as_f64()
        {
            self.quantiles.insert(f);
        }
        // Typed min/max.
        self.update_min_max(v);
    }

    fn update_min_max(&mut self, v: &Value) {
        if let Some(cur) = &self.min {
            if value_lt(v, cur) {
                self.min = Some(v.clone());
            }
        } else {
            self.min = Some(v.clone());
        }
        if let Some(cur) = &self.max {
            if value_lt(cur, v) {
                self.max = Some(v.clone());
            }
        } else {
            self.max = Some(v.clone());
        }
    }

    /// Merge already-written PAX zone-map bounds for this field (no rescan).
    fn merge_zone_bounds(&mut self, min: Option<&Value>, max: Option<&Value>) {
        if let Some(m) = min {
            self.update_min_max(m);
        }
        if let Some(m) = max {
            self.update_min_max(m);
        }
    }

    fn to_field_statistics(&self) -> FieldStatistics {
        let mut qd = self.quantiles.clone();
        let quantiles = if self.numeric && !qd.is_empty() {
            let mut q = BTreeMap::new();
            for &p in &EMITTED_QUANTILES {
                if let Some(val) = qd.quantile(p) {
                    q.insert(format!("{p}"), val);
                }
            }
            Some(Quantiles {
                method: "tdigest".to_string(),
                q,
                buckets: Vec::new(),
            })
        } else {
            None
        };

        let null_rate = if self.total > 0 {
            Some(self.nulls as f64 / self.total as f64)
        } else {
            None
        };

        let (distinct_estimate, distinct_method) = if self.distinct.is_empty() {
            (None, None)
        } else {
            (Some(self.distinct.estimate()), Some("hll".to_string()))
        };

        FieldStatistics {
            name: self.name.clone(),
            data_type: self.data_type.clone(),
            null_rate,
            distinct_estimate,
            distinct_method,
            min: self.min.clone(),
            max: self.max.clone(),
            quantiles,
            examples: self
                .examples
                .samples()
                .iter()
                .map(|s| Value::String(s.clone()))
                .collect(),
            indexed: self.indexed,
            filterable: self.filterable,
            approximate: self.approximate,
        }
    }
}

/// Document-modality accumulator (BM25 corpus stats; TD-175).
#[derive(Debug, Clone, Default)]
struct DocAccumulator {
    doc_count: u64,
    length_sum: u64,
    unique_terms: HyperLogLog,
    top_terms: Option<FrequentItems>,
    /// term -> doc-frequency for idf; tracked via the frequent-items sketch.
    df: Option<FrequentItems>,
}

/// Vector-modality accumulator (centroid/spread/occupancy; TD-175).
#[derive(Debug, Clone, Default)]
struct VectorAccumulator {
    dimension: u32,
    distance_metric: String,
    count: u64,
    centroid_sum: Vec<f64>,
    /// Sum of L2 distance to the *running* centroid — an online spread proxy.
    spread_sum: f64,
    cluster_occupancy: Vec<u64>,
}

/// Resident per-collection statistics. Mergeable across segments at compaction.
#[derive(Debug, Clone)]
pub struct StatisticsSummary {
    collection_id: String,
    freshness: Option<Freshness>,
    record_count: u64,
    storage_size_bytes: u64,
    index_size_bytes: Option<u64>,
    fields: BTreeMap<String, FieldAccumulator>,
    doc: Option<DocAccumulator>,
    vector: Option<VectorAccumulator>,
    graph: Option<GraphStatistics>,
    trace: Option<TraceStatistics>,
    timeseries: Option<TimeseriesStatistics>,
}

impl StatisticsSummary {
    pub fn new(collection_id: impl Into<String>) -> Self {
        Self {
            collection_id: collection_id.into(),
            freshness: None,
            record_count: 0,
            storage_size_bytes: 0,
            index_size_bytes: None,
            fields: BTreeMap::new(),
            doc: None,
            vector: None,
            graph: None,
            trace: None,
            timeseries: None,
        }
    }

    /// Stamp the freshness watermark — a FACT advanced at flush/compaction
    /// (`source` is `"flush"` or `"compaction"`). Never an SLA.
    pub fn set_freshness(
        &mut self,
        as_of: impl Into<String>,
        source: impl Into<String>,
        segment_watermark: Option<u64>,
    ) {
        self.freshness = Some(Freshness {
            as_of: as_of.into(),
            source: source.into(),
            segment_watermark,
        });
    }

    /// The collection this summary describes.
    pub fn collection_id(&self) -> &str {
        &self.collection_id
    }

    pub fn set_record_count(&mut self, n: u64) {
        self.record_count = n;
    }

    pub fn set_sizes(&mut self, storage_size_bytes: u64, index_size_bytes: Option<u64>) {
        self.storage_size_bytes = storage_size_bytes;
        self.index_size_bytes = index_size_bytes;
    }

    /// Record schema-level facts for a field (whether or not values are observed).
    pub fn set_field_schema(
        &mut self,
        name: &str,
        data_type: Value,
        indexed: Option<bool>,
        filterable: Option<bool>,
    ) {
        let acc = self
            .fields
            .entry(name.to_string())
            .or_insert_with(|| FieldAccumulator::new(name.to_string(), data_type.clone()));
        acc.indexed = indexed;
        acc.filterable = filterable;
    }

    /// Observe one record's value for a field (the write-boundary feed).
    pub fn observe_field(&mut self, name: &str, data_type: &Value, value: Option<&Value>) {
        let acc = self
            .fields
            .entry(name.to_string())
            .or_insert_with(|| FieldAccumulator::new(name.to_string(), data_type.clone()));
        acc.observe(value);
    }

    /// Merge already-written PAX zone-map bounds for a field (no rescan).
    pub fn merge_zone_bounds(
        &mut self,
        name: &str,
        data_type: &Value,
        min: Option<&Value>,
        max: Option<&Value>,
    ) {
        let acc = self
            .fields
            .entry(name.to_string())
            .or_insert_with(|| FieldAccumulator::new(name.to_string(), data_type.clone()));
        acc.merge_zone_bounds(min, max);
    }

    // ---- modality feeds (TD-175) ------------------------------------------

    pub fn set_vector_meta(&mut self, dimension: u32, distance_metric: impl Into<String>) {
        let v = self.vector.get_or_insert_with(VectorAccumulator::default);
        v.dimension = dimension;
        v.distance_metric = distance_metric.into();
    }

    /// Observe a vector for centroid/spread accumulation.
    pub fn observe_vector(&mut self, embedding: &[f32]) {
        let v = self.vector.get_or_insert_with(VectorAccumulator::default);
        if v.centroid_sum.len() != embedding.len() {
            v.centroid_sum = vec![0.0; embedding.len()];
        }
        v.count += 1;
        let mut sq = 0.0_f64;
        for (acc, &x) in v.centroid_sum.iter_mut().zip(embedding.iter()) {
            *acc += x as f64;
        }
        // Spread proxy: distance to the running centroid mean.
        if v.count > 1 {
            for (i, &x) in embedding.iter().enumerate() {
                let mean = v.centroid_sum[i] / v.count as f64;
                let d = x as f64 - mean;
                sq += d * d;
            }
            v.spread_sum += sq.sqrt();
        }
    }

    pub fn observe_term(&mut self, term: &str) {
        let d = self.doc.get_or_insert_with(DocAccumulator::default);
        d.unique_terms.insert_bytes(term.as_bytes());
        d.top_terms
            .get_or_insert_with(|| FrequentItems::new(TOP_TERMS_CAP))
            .insert(term);
    }

    /// Observe a document: its length and the set of distinct terms it contains
    /// (for document-frequency / idf).
    pub fn observe_document(&mut self, length: u64, distinct_terms: &[String]) {
        let d = self.doc.get_or_insert_with(DocAccumulator::default);
        d.doc_count += 1;
        d.length_sum += length;
        let df =
            d.df.get_or_insert_with(|| FrequentItems::new(TOP_TERMS_CAP));
        for t in distinct_terms {
            df.insert(t);
        }
    }

    pub fn set_graph_stats(&mut self, g: GraphStatistics) {
        self.graph = Some(g);
    }

    pub fn set_trace_stats(&mut self, t: TraceStatistics) {
        self.trace = Some(t);
    }

    pub fn set_timeseries_stats(&mut self, t: TimeseriesStatistics) {
        self.timeseries = Some(t);
    }

    /// Selectivity estimate for `field = ?` equality, from the distinct sketch
    /// (feeds ADR-004's `estimated_selectivity`). `None` if unknown.
    pub fn equality_selectivity(&self, field: &str) -> Option<f64> {
        let acc = self.fields.get(field)?;
        if acc.distinct.is_empty() {
            return None;
        }
        let d = acc.distinct.estimate();
        if d == 0 { None } else { Some(1.0 / d as f64) }
    }

    /// Project the frozen v1 envelope. Always available (no rescan).
    pub fn to_envelope(&self) -> StatisticsEnvelope {
        let freshness = self.freshness.clone().unwrap_or_else(|| Freshness {
            // Never fabricate a watermark: if no flush has been observed, attest
            // the unknown honestly as an empty flush watermark at epoch. The
            // consumer treats a zero/empty watermark as "no snapshot yet".
            as_of: "1970-01-01T00:00:00Z".to_string(),
            source: "flush".to_string(),
            segment_watermark: None,
        });

        let fields: Vec<FieldStatistics> = self
            .fields
            .values()
            .map(|a| a.to_field_statistics())
            .collect();

        let mut modality: Vec<ModalityStatistics> = Vec::new();
        if let Some(d) = &self.doc {
            modality.push(ModalityStatistics::Document(self.document_block(d)));
        }
        if let Some(v) = &self.vector {
            modality.push(ModalityStatistics::Vector(self.vector_block(v)));
        }
        if let Some(g) = &self.graph {
            modality.push(ModalityStatistics::Graph(g.clone()));
        }
        if let Some(t) = &self.trace {
            modality.push(ModalityStatistics::Trace(t.clone()));
        }
        if let Some(t) = &self.timeseries {
            modality.push(ModalityStatistics::Timeseries(t.clone()));
        }

        StatisticsEnvelope {
            envelope_version: ENVELOPE_VERSION.to_string(),
            collection_id: self.collection_id.clone(),
            freshness,
            record_count: self.record_count,
            storage_size_bytes: self.storage_size_bytes,
            index_size_bytes: self.index_size_bytes,
            fields,
            modality,
        }
    }

    fn document_block(&self, d: &DocAccumulator) -> DocumentStatistics {
        let avg_doc_length = if d.doc_count > 0 {
            Some(d.length_sum as f64 / d.doc_count as f64)
        } else {
            None
        };
        let unique_terms_estimate = if d.unique_terms.is_empty() {
            None
        } else {
            Some(d.unique_terms.estimate())
        };
        let top_terms =
            d.df.as_ref()
                .or(d.top_terms.as_ref())
                .map(|f| {
                    f.top(TOP_TERMS_CAP)
                        .into_iter()
                        .map(|it| TermStat {
                            term: it.item,
                            doc_frequency: it.count,
                            idf: idf(d.doc_count, it.count),
                        })
                        .collect()
                })
                .unwrap_or_default();
        DocumentStatistics {
            doc_count: d.doc_count,
            avg_doc_length,
            unique_terms_estimate,
            top_terms,
        }
    }

    fn vector_block(&self, v: &VectorAccumulator) -> VectorStatistics {
        let centroid = if v.count > 0 && !v.centroid_sum.is_empty() {
            v.centroid_sum.iter().map(|s| s / v.count as f64).collect()
        } else {
            Vec::new()
        };
        let spread = if v.count > 1 {
            Some(v.spread_sum / (v.count - 1) as f64)
        } else {
            None
        };
        VectorStatistics {
            dimension: v.dimension,
            distance_metric: v.distance_metric.clone(),
            spread,
            centroid,
            cluster_occupancy: v.cluster_occupancy.clone(),
            // Centroid/spread are running estimates over a sample → approximate.
            approximate: v.count > 0,
        }
    }
}

/// Inverse document frequency (smoothed, standard form). Returns 0 when undefined.
fn idf(doc_count: u64, df: u64) -> f64 {
    if doc_count == 0 || df == 0 {
        return 0.0;
    }
    let n = doc_count as f64;
    let d = df as f64;
    (((n - d + 0.5) / (d + 0.5)) + 1.0).ln()
}

/// True for ProximaType serde forms that carry an orderable numeric value.
fn is_numeric_type(data_type: &Value) -> bool {
    match data_type {
        Value::String(s) => matches!(
            s.as_str(),
            "Int8"
                | "Int16"
                | "Int32"
                | "Int64"
                | "UInt8"
                | "UInt16"
                | "UInt32"
                | "UInt64"
                | "Float32"
                | "Float64"
        ),
        _ => false,
    }
}

/// Canonical string for a typed value (HLL/reservoir/frequent keys + examples).
fn canonical_string(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

/// Typed less-than over JSON values: numbers numerically, strings lexically,
/// bools false<true; mixed/other types are incomparable (returns false → no
/// min/max update, which is safe).
fn value_lt(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Number(_), Value::Number(_)) => match (a.as_f64(), b.as_f64()) {
            (Some(x), Some(y)) => x < y,
            _ => false,
        },
        (Value::String(x), Value::String(y)) => x < y,
        (Value::Bool(x), Value::Bool(y)) => !x && *y,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &str) -> Value {
        Value::String(v.to_string())
    }
    fn n(v: i64) -> Value {
        Value::Number(v.into())
    }

    #[test]
    fn assembles_top_level_units() {
        let mut sum = StatisticsSummary::new("incidents");
        sum.set_record_count(100);
        sum.set_sizes(4096, Some(1024));
        sum.set_freshness("2026-06-26T12:00:00Z", "compaction", Some(42));
        let env = sum.to_envelope();
        assert_eq!(env.envelope_version, ENVELOPE_VERSION);
        assert_eq!(env.collection_id, "incidents");
        assert_eq!(env.record_count, 100);
        assert_eq!(env.storage_size_bytes, 4096);
        assert_eq!(env.index_size_bytes, Some(1024));
        assert_eq!(env.freshness.source, "compaction");
        assert_eq!(env.freshness.segment_watermark, Some(42));
    }

    #[test]
    fn field_null_rate_and_minmax_and_distinct() {
        let mut sum = StatisticsSummary::new("c");
        let ty = s("Int64");
        // 8 values, 2 null.
        for v in [
            Some(n(5)),
            Some(n(1)),
            None,
            Some(n(9)),
            Some(n(3)),
            None,
            Some(n(7)),
            Some(n(2)),
        ] {
            sum.observe_field("age", &ty, v.as_ref());
        }
        let env = sum.to_envelope();
        let f = env.fields.iter().find(|f| f.name == "age").unwrap();
        assert!((f.null_rate.unwrap() - 0.25).abs() < 1e-9);
        assert_eq!(f.min, Some(n(1)));
        assert_eq!(f.max, Some(n(9)));
        assert_eq!(f.distinct_method.as_deref(), Some("hll"));
        assert!(f.approximate);
        assert!(f.quantiles.is_some(), "numeric field has quantiles");
    }

    #[test]
    fn string_field_minmax_lexical_no_quantiles() {
        let mut sum = StatisticsSummary::new("c");
        let ty = s("String");
        for v in ["banana", "apple", "cherry"] {
            sum.observe_field("name", &ty, Some(&s(v)));
        }
        let env = sum.to_envelope();
        let f = env.fields.iter().find(|f| f.name == "name").unwrap();
        assert_eq!(f.min, Some(s("apple")));
        assert_eq!(f.max, Some(s("cherry")));
        assert!(f.quantiles.is_none(), "string field has no quantiles");
        assert!(!f.examples.is_empty());
    }

    #[test]
    fn zone_bounds_merge_without_observation() {
        let mut sum = StatisticsSummary::new("c");
        let ty = s("Int64");
        sum.merge_zone_bounds("age", &ty, Some(&n(1)), Some(&n(1000)));
        sum.merge_zone_bounds("age", &ty, Some(&n(-5)), Some(&n(500)));
        let env = sum.to_envelope();
        let f = env.fields.iter().find(|f| f.name == "age").unwrap();
        assert_eq!(f.min, Some(n(-5)));
        assert_eq!(f.max, Some(n(1000)));
    }

    #[test]
    fn document_and_vector_and_graph_blocks() {
        let mut sum = StatisticsSummary::new("c");
        sum.set_vector_meta(4, "cosine");
        sum.observe_vector(&[1.0, 0.0, 0.0, 0.0]);
        sum.observe_vector(&[0.0, 1.0, 0.0, 0.0]);
        sum.observe_document(3, &["timeout".into(), "payment".into(), "error".into()]);
        sum.observe_document(2, &["timeout".into(), "retry".into()]);
        sum.set_graph_stats(GraphStatistics {
            total_nodes: 10,
            total_edges: 20,
            average_degree: Some(4.0),
            max_degree: Some(8),
            connected_components: Some(2),
            label_counts: Vec::new(),
            edge_type_counts: Vec::new(),
        });
        let env = sum.to_envelope();
        assert_eq!(env.modality.len(), 3);
        let has_doc = env
            .modality
            .iter()
            .any(|m| matches!(m, ModalityStatistics::Document(d) if d.doc_count == 2));
        let has_vec = env
            .modality
            .iter()
            .any(|m| matches!(m, ModalityStatistics::Vector(v) if v.dimension == 4));
        let has_graph = env
            .modality
            .iter()
            .any(|m| matches!(m, ModalityStatistics::Graph(g) if g.total_nodes == 10));
        assert!(has_doc && has_vec && has_graph);
    }

    #[test]
    fn selectivity_from_distinct() {
        let mut sum = StatisticsSummary::new("c");
        let ty = s("Int64");
        for i in 0..100 {
            sum.observe_field("id", &ty, Some(&n(i)));
        }
        let sel = sum.equality_selectivity("id").unwrap();
        // ~1/100; HLL is approximate so allow a band.
        assert!(sel > 0.005 && sel < 0.02, "selectivity {sel}");
        assert!(sum.equality_selectivity("missing").is_none());
    }

    #[test]
    fn envelope_round_trips_to_json() {
        let mut sum = StatisticsSummary::new("c");
        sum.set_record_count(1);
        sum.set_sizes(10, None);
        sum.observe_field("x", &s("Int64"), Some(&n(7)));
        let env = sum.to_envelope();
        let json = serde_json::to_string(&env).unwrap();
        let back: StatisticsEnvelope = serde_json::from_str(&json).unwrap();
        assert_eq!(env, back);
    }
}
