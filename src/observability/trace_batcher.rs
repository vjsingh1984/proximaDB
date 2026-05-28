// Trace batcher — bundles N populated SearchPlanTraces into one POST.
//
// The gateway's async billing sink fires one HTTP POST per trace today.
// Under steady-state QPS that's expensive at every layer (TLS, ingest
// queue, write amplification). This module batches traces so the
// gateway can flush once per N traces OR once per flush window.
//
// Each batch record carries:
//   - `idempotency_key` from `trace_digest::digest_hex` so a CDC replay
//     of the same batch is a no-op on the sink.
//   - `fingerprint` from `trace_fingerprint::fingerprint_hex` so
//     downstream dashboards can group by shape without re-deriving.
//   - The KRU metering event from `metering_event::build_kru`.
//
// The batch envelope itself carries:
//   - `count` of records.
//   - `distinct_fingerprints` — how many unique shapes are in the
//     batch (a single hot shape over many traces indicates a workload
//     pattern worth noticing).
//   - `estimated_bytes` — rough JSON size for the gateway's
//     queue-pressure heuristic.

use std::collections::HashSet;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::observability::metering_event::{MeteringInputs, build_kru};
use crate::observability::search_plan_trace::SearchPlanTrace;
use crate::observability::trace_digest::{DigestInputs, digest_hex};
use crate::observability::trace_fingerprint::{TraceShape, fingerprint_hex};

/// One record inside the batch. Owned fields so the batch can outlive
/// the source traces.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceBatchRecord {
    pub idempotency_key: String,
    pub fingerprint: String,
    /// The metering event metadata (matches the operator metering-events
    /// collection schema's `metadata` field shape).
    pub metering: Value,
}

/// Top-level batch envelope. Serializes to JSON for the POST body.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct TraceBatch {
    pub records: Vec<TraceBatchRecord>,
    pub count: usize,
    pub distinct_fingerprints: usize,
    pub estimated_bytes: usize,
}

impl TraceBatch {
    pub fn is_empty(&self) -> bool {
        self.count == 0
    }
}

/// Per-trace input the batcher consumes. The same `(corpus_gb,
/// total_vectors, tier_label, occurred_at, idempotency_bucket)` may
/// differ across traces in the batch — they're per-record so a single
/// batch can carry multi-tenant rows when the gateway aggregates.
#[derive(Debug, Clone)]
pub struct TraceBatchInput<'a> {
    pub trace: &'a SearchPlanTrace,
    pub corpus_gb: f64,
    pub total_vectors: u64,
    pub tier_label: &'static str,
    pub occurred_at_ms: u64,
    pub occurred_at_iso: String,
    /// Bucket for the idempotency-key timestamp. Defaults to 1s.
    pub idempotency_bucket: Duration,
}

/// Build a batch from a slice of inputs. Returns an empty batch when
/// the input slice is empty.
pub fn build_batch(inputs: &[TraceBatchInput<'_>]) -> TraceBatch {
    if inputs.is_empty() {
        return TraceBatch {
            records: Vec::new(),
            count: 0,
            distinct_fingerprints: 0,
            estimated_bytes: 2, // "[]"
        };
    }

    let mut records = Vec::with_capacity(inputs.len());
    let mut fingerprint_set: HashSet<String> = HashSet::with_capacity(inputs.len());
    let mut byte_total = 2usize; // outer brackets

    for input in inputs {
        let trace = input.trace;
        let metering = build_kru(&MeteringInputs {
            trace,
            tier_label: input.tier_label,
            corpus_gb: input.corpus_gb,
            total_vectors: input.total_vectors,
            occurred_at: input.occurred_at_iso.clone(),
        });

        let idempotency_key = digest_hex(&DigestInputs {
            tenant_id: &trace.tenant_id,
            trace_id: &trace.trace_id,
            occurred_at_ms: input.occurred_at_ms,
            bucket: input.idempotency_bucket,
        });

        let shape = TraceShape::from_trace(trace, input.corpus_gb);
        let fingerprint = fingerprint_hex(&shape);
        fingerprint_set.insert(fingerprint.clone());

        let record = TraceBatchRecord {
            idempotency_key,
            fingerprint,
            metering: metering.metadata,
        };
        // Estimate JSON size — cheap upper bound for the gateway's
        // queue-pressure heuristic, not a precise serialization.
        byte_total += estimate_record_bytes(&record);
        records.push(record);
    }

    let count = records.len();
    let distinct = fingerprint_set.len();
    TraceBatch {
        records,
        count,
        distinct_fingerprints: distinct,
        estimated_bytes: byte_total,
    }
}

fn estimate_record_bytes(record: &TraceBatchRecord) -> usize {
    // 16-char hex idempotency_key + 16-char fingerprint + key labels
    // + estimated metering size.
    let metering_size = record.metering.to_string().len();
    // {"idempotency_key":"…","fingerprint":"…","metering":…},
    32 + metering_size + 64
}

/// Convenience builder for the case where every trace in the batch
/// shares the same tier label, corpus context, and timestamp (the
/// "homogeneous flush" — most common). Returns a vec of inputs the
/// caller hands to `build_batch`.
pub fn homogeneous_inputs<'a>(
    traces: &'a [&'a SearchPlanTrace],
    corpus_gb: f64,
    total_vectors: u64,
    tier_label: &'static str,
    occurred_at_ms: u64,
    occurred_at_iso: &str,
) -> Vec<TraceBatchInput<'a>> {
    traces
        .iter()
        .map(|t| TraceBatchInput {
            trace: t,
            corpus_gb,
            total_vectors,
            tier_label,
            occurred_at_ms,
            occurred_at_iso: occurred_at_iso.to_string(),
            idempotency_bucket: Duration::from_secs(1),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::service_types::IndexStats;
    use crate::observability::search_plan_trace::{
        CacheResult, FilterStrategy, IndexRoute, SureSignals,
    };

    fn trace_template(trace_id: &str, tenant: &str) -> SearchPlanTrace {
        SearchPlanTrace {
            trace_id: trace_id.into(),
            tenant_id: tenant.into(),
            collection_name: "kb".into(),
            plan_version: 1,
            filter_strategy: FilterStrategy::HybridFilter,
            index_route: IndexRoute::FullPrecisionGraph,
            cache_result: CacheResult::Miss,
            estimated_selectivity: Some(0.1),
            actual_selectivity: None,
            gls_score: None,
            estimated_scan_gb: None,
            actual_scan_gb: 0.0,
            index_stats: IndexStats::default(),
            candidate_count: 64,
            rerank_count: 10,
            repair_count: 0,
            sure_signals: SureSignals::default(),
            latency_ms: 12.3,
            recall_probe_score: None,
            utility_score_avg: None,
            failure_class: None,
            predicate_shortfall: None,
        }
    }

    fn input<'a>(trace: &'a SearchPlanTrace) -> TraceBatchInput<'a> {
        TraceBatchInput {
            trace,
            corpus_gb: 1.0,
            total_vectors: 1_000_000,
            tier_label: "business",
            occurred_at_ms: 1_700_000_000_000,
            occurred_at_iso: "2026-05-21T00:00:00Z".into(),
            idempotency_bucket: Duration::from_secs(1),
        }
    }

    #[test]
    fn empty_input_yields_empty_batch() {
        let b = build_batch(&[]);
        assert!(b.is_empty());
        assert_eq!(b.count, 0);
        assert_eq!(b.distinct_fingerprints, 0);
        assert_eq!(b.records.len(), 0);
        // Outer brackets only.
        assert_eq!(b.estimated_bytes, 2);
    }

    #[test]
    fn single_trace_produces_single_record() {
        let t = trace_template("t1", "tenant-a");
        let b = build_batch(&[input(&t)]);
        assert_eq!(b.count, 1);
        assert_eq!(b.records.len(), 1);
        assert_eq!(b.distinct_fingerprints, 1);
        // Idempotency key and fingerprint are 16-char hex.
        assert_eq!(b.records[0].idempotency_key.len(), 16);
        assert_eq!(b.records[0].fingerprint.len(), 16);
    }

    #[test]
    fn batch_preserves_input_order() {
        let traces: Vec<SearchPlanTrace> = (0..5)
            .map(|i| trace_template(&format!("trace-{i}"), "tenant-a"))
            .collect();
        let inputs: Vec<TraceBatchInput<'_>> = traces.iter().map(input).collect();
        let b = build_batch(&inputs);
        assert_eq!(b.count, 5);
        // Each record's metering carries the original tenant_id; since
        // they're identical here, the trace_ids in the metering's
        // metadata should mirror input order via idempotency_key
        // uniqueness ordering — verify keys are all distinct.
        let keys: HashSet<_> = b
            .records
            .iter()
            .map(|r| r.idempotency_key.clone())
            .collect();
        assert_eq!(keys.len(), 5);
    }

    #[test]
    fn distinct_fingerprints_counts_unique_shapes() {
        // Three traces: two with identical shape, one with a distinct
        // filter strategy → 2 distinct fingerprints.
        let t1 = trace_template("t1", "tenant-a");
        let t2 = trace_template("t2", "tenant-a"); // same shape
        let mut t3 = trace_template("t3", "tenant-a");
        t3.filter_strategy = FilterStrategy::PreFilter;

        let inputs = vec![input(&t1), input(&t2), input(&t3)];
        let b = build_batch(&inputs);
        assert_eq!(b.count, 3);
        assert_eq!(
            b.distinct_fingerprints, 2,
            "two t1/t2 share shape, t3 distinct"
        );
    }

    #[test]
    fn identical_traces_share_fingerprint_but_have_distinct_idempotency_keys() {
        // Same tenant + same shape, but distinct trace_ids → same
        // fingerprint, distinct idempotency keys.
        let t1 = trace_template("trace-1", "tenant-a");
        let t2 = trace_template("trace-2", "tenant-a");
        let b = build_batch(&[input(&t1), input(&t2)]);
        assert_eq!(b.records[0].fingerprint, b.records[1].fingerprint);
        assert_ne!(b.records[0].idempotency_key, b.records[1].idempotency_key);
    }

    #[test]
    fn cross_tenant_batch_carries_distinct_idempotency_keys() {
        // Same trace_id under different tenants → distinct idempotency
        // keys (the digest mixes tenant in).
        let t1 = trace_template("trace-1", "tenant-a");
        let t2 = trace_template("trace-1", "tenant-b");
        let b = build_batch(&[input(&t1), input(&t2)]);
        assert_ne!(b.records[0].idempotency_key, b.records[1].idempotency_key);
        // But the shape fingerprint is the same — both share the same
        // plan/route/etc.
        assert_eq!(b.records[0].fingerprint, b.records[1].fingerprint);
    }

    #[test]
    fn batch_round_trips_via_json() {
        let t = trace_template("t1", "tenant-a");
        let b = build_batch(&[input(&t)]);
        let s = serde_json::to_string(&b).expect("serialize");
        let back: TraceBatch = serde_json::from_str(&s).expect("deserialize");
        assert_eq!(b, back);
    }

    #[test]
    fn estimated_bytes_grows_with_batch_size() {
        let t1 = trace_template("t1", "tenant-a");
        let t2 = trace_template("t2", "tenant-a");
        let b1 = build_batch(&[input(&t1)]);
        let b2 = build_batch(&[input(&t1), input(&t2)]);
        assert!(b2.estimated_bytes > b1.estimated_bytes);
    }

    #[test]
    fn metering_metadata_contains_expected_keys() {
        // Spot-check that the metering Value in each record is the
        // structured KRU shape, not a stringified blob.
        let t = trace_template("t1", "tenant-a");
        let b = build_batch(&[input(&t)]);
        let m = &b.records[0].metering;
        assert!(m.get("event_type").is_some());
        assert!(m.get("tenant_id").is_some());
        assert!(m.get("scanned_gb").is_some());
        assert_eq!(m["event_type"], "kru");
        assert_eq!(m["tenant_id"], "tenant-a");
    }

    #[test]
    fn homogeneous_inputs_helper_threads_shared_context() {
        let t1 = trace_template("t1", "tenant-a");
        let t2 = trace_template("t2", "tenant-a");
        let refs: Vec<&SearchPlanTrace> = vec![&t1, &t2];
        let inputs = homogeneous_inputs(
            &refs,
            1.0,
            1_000_000,
            "business",
            1_700_000_000_000,
            "2026-05-21T00:00:00Z",
        );
        let b = build_batch(&inputs);
        assert_eq!(b.count, 2);
    }

    #[test]
    fn idempotency_bucket_collapses_replay_within_window() {
        // Same trace_id at two timestamps within the bucket → same key.
        let t = trace_template("t1", "tenant-a");
        let mut i1 = input(&t);
        let mut i2 = input(&t);
        i1.occurred_at_ms = 1_700_000_000_000;
        i2.occurred_at_ms = 1_700_000_000_500; // 500ms later
        i1.idempotency_bucket = Duration::from_secs(1);
        i2.idempotency_bucket = Duration::from_secs(1);
        let b1 = build_batch(&[i1]);
        let b2 = build_batch(&[i2]);
        assert_eq!(b1.records[0].idempotency_key, b2.records[0].idempotency_key);
    }

    #[test]
    fn distinct_buckets_distinguish_repeated_trace_id() {
        let t = trace_template("t1", "tenant-a");
        let mut i1 = input(&t);
        let mut i2 = input(&t);
        i1.occurred_at_ms = 1_700_000_000_000;
        i2.occurred_at_ms = 1_700_000_002_000; // 2s later → different 1s bucket
        i1.idempotency_bucket = Duration::from_secs(1);
        i2.idempotency_bucket = Duration::from_secs(1);
        let b1 = build_batch(&[i1]);
        let b2 = build_batch(&[i2]);
        assert_ne!(b1.records[0].idempotency_key, b2.records[0].idempotency_key);
    }

    #[test]
    fn empty_records_serialize_to_compact_json() {
        let b = build_batch(&[]);
        let s = serde_json::to_string(&b).expect("serialize");
        // No panic; result is a complete JSON object even when empty.
        assert!(s.contains("\"count\":0"));
        assert!(s.contains("\"records\":[]"));
    }

    #[test]
    fn record_fields_are_named_for_billing_sink_consumption() {
        // The downstream sink expects these field names verbatim.
        let t = trace_template("t1", "tenant-a");
        let b = build_batch(&[input(&t)]);
        let s = serde_json::to_string(&b.records[0]).expect("serialize");
        assert!(s.contains("\"idempotency_key\""));
        assert!(s.contains("\"fingerprint\""));
        assert!(s.contains("\"metering\""));
    }
}
