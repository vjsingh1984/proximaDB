// Trace pipeline integration — composes the retrieval-cost primitives
// end-to-end and asserts the wire shape.
//
// This is the first integration-level proof that the primitive surface
// composes:
//
//   PlanBuilder → PlanOutput
//        ↓
//   TraceBuilder → SearchPlanTrace
//        ↓
//   metering_event::build_kru → MeteringEvent (KRU JSON)
//        ↓
//   trace_digest::digest_hex   (idempotency key)
//   trace_fingerprint::fingerprint_hex (shape group)
//        ↓
//   trace_batcher::build_batch → TraceBatch (ready to POST)
//
// The test doesn't replace the per-primitive unit tests — it proves
// the *interfaces* between them line up. A future change that breaks
// the composition (e.g. a field rename in SearchPlanTrace) trips this
// test even if every per-primitive test still passes.

use std::time::Duration;

use proximadb::catalog::tenant_tier::{Tier, TenantTierRecord};
use proximadb::core::service_types::IndexStats;
use proximadb::observability::metering_event::{MeteringInputs, build_kru};
use proximadb::observability::search_plan_trace::{
    CacheResult, FilterStrategy, IndexRoute, SearchPlanTrace, SureSignals,
};
use proximadb::observability::search_plan_trace_builder::{TraceBuilderInputs, build as build_trace};
use proximadb::observability::trace_batcher::{TraceBatchInput, build_batch};
use proximadb::observability::trace_digest::{DigestInputs, digest_hex};
use proximadb::observability::trace_fingerprint::{TraceShape, fingerprint_hex};
use proximadb::query::federated::optimizer::filter_strategy::PlanInputs;
use proximadb::query::federated::optimizer::plan_builder::{PlanBuilderInputs, build_for_search};
use proximadb::query::federated::optimizer::selectivity::FieldStatistics;
use proximadb::query::federated::optimizer::PredicateSelectivityPolicy;

/// One end-to-end pass — every stage of the pipeline runs against a
/// realistic query shape. The test asserts cross-stage invariants:
///   1. The plan that comes out of PlanBuilder matches what
///      TraceBuilder embeds.
///   2. The metering event's `tenant_id` and `event_type` flow
///      through unchanged.
///   3. The trace_digest and trace_fingerprint produce the
///      16-char hex shape the batcher expects.
///   4. The batcher's distinct_fingerprints counter agrees with
///      what we built.
#[test]
fn full_pipeline_composes_for_a_single_trace() {
    // Stage 1: plan.
    let tier = TenantTierRecord::fail_safe("tenant-a");
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let plan = build_for_search(&PlanBuilderInputs {
        predicates: &[],
        field_stats: &stats,
        policy: &policy,
        gls_samples: &[],
        dim: 768,
        recall_target: 0.9,
        collection_gb: 0.1,
        tier: &tier,
    });
    // Empty predicates → PostFilter / FullPrecisionGraph (LLD §3 bands).
    assert_eq!(plan.filter_strategy, FilterStrategy::PostFilter);
    assert_eq!(plan.index_route, IndexRoute::FullPrecisionGraph);

    // Stage 2: trace.
    let trace = build_trace(TraceBuilderInputs {
        trace_id: "trace-int-1".into(),
        tenant_id: "tenant-a".into(),
        collection_name: "kb".into(),
        plan: &plan,
        latency_ms: 12.3,
        index_stats: {
            let mut s = IndexStats::default();
            s.vectors_scanned = 100;
            s
        },
        candidate_count: 64,
        rerank_count: 10,
        repair_count: 0,
        sure_signals: SureSignals::default(),
        cache_result: CacheResult::Miss,
        failure_class: None,
        bytes_per_vector: 1024.0,
    });
    // The trace's plan must round-trip from PlanOutput.
    assert_eq!(trace.filter_strategy, plan.filter_strategy);
    assert_eq!(trace.index_route, plan.index_route);
    // Identity fields preserved.
    assert_eq!(trace.trace_id, "trace-int-1");
    assert_eq!(trace.tenant_id, "tenant-a");
    assert_eq!(trace.collection_name, "kb");
    // actual_scan_gb derived from vectors_scanned × bytes_per_vector.
    assert!(trace.actual_scan_gb > 0.0);

    // Stage 3: metering event.
    let metering = build_kru(&MeteringInputs {
        trace: &trace,
        tier_label: Tier::FreeTrial.prometheus_label(),
        corpus_gb: 1.0,
        total_vectors: 1_000_000,
        occurred_at: "2026-05-22T00:00:00Z".into(),
    });
    assert_eq!(metering.event_type, "kru");
    assert_eq!(metering.tenant_id, "tenant-a");
    assert!(metering.quantity > 0.0);
    // The metering metadata carries the trace's tenant + event type.
    assert_eq!(metering.metadata["tenant_id"], "tenant-a");
    assert_eq!(metering.metadata["event_type"], "kru");
    assert_eq!(metering.metadata["tier"], "free");

    // Stage 4: digest + fingerprint.
    let digest = digest_hex(&DigestInputs {
        tenant_id: &trace.tenant_id,
        trace_id: &trace.trace_id,
        occurred_at_ms: 1_700_000_000_000,
        bucket: Duration::from_secs(1),
    });
    let shape = TraceShape::from_trace(&trace, 1.0);
    let fp = fingerprint_hex(&shape);
    assert_eq!(digest.len(), 16);
    assert_eq!(fp.len(), 16);
    // Digest and fingerprint serve different purposes — distinct
    // values for the same trace.
    assert_ne!(digest, fp);

    // Stage 5: batcher.
    let batch_input = TraceBatchInput {
        trace: &trace,
        corpus_gb: 1.0,
        total_vectors: 1_000_000,
        tier_label: "free",
        occurred_at_ms: 1_700_000_000_000,
        occurred_at_iso: "2026-05-22T00:00:00Z".into(),
        idempotency_bucket: Duration::from_secs(1),
    };
    let batch = build_batch(&[batch_input]);
    assert_eq!(batch.count, 1);
    assert_eq!(batch.distinct_fingerprints, 1);
    assert_eq!(batch.records[0].idempotency_key, digest);
    assert_eq!(batch.records[0].fingerprint, fp);
    assert_eq!(batch.records[0].metering["event_type"], "kru");
}

/// Multi-trace batch: two traces with identical shape but distinct
/// trace_ids must produce one distinct fingerprint and two distinct
/// idempotency keys. Exercises the same invariants the individual
/// primitive tests cover, but at the integration boundary.
#[test]
fn batch_of_identical_shapes_collapses_fingerprint_but_keeps_idempotency_distinct() {
    let tier = TenantTierRecord::fail_safe("tenant-a");
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let plan = build_for_search(&PlanBuilderInputs {
        predicates: &[],
        field_stats: &stats,
        policy: &policy,
        gls_samples: &[],
        dim: 768,
        recall_target: 0.9,
        collection_gb: 0.1,
        tier: &tier,
    });

    let make_trace = |id: &str| -> SearchPlanTrace {
        build_trace(TraceBuilderInputs {
            trace_id: id.into(),
            tenant_id: "tenant-a".into(),
            collection_name: "kb".into(),
            plan: &plan,
            latency_ms: 12.3,
            index_stats: IndexStats::default(),
            candidate_count: 64,
            rerank_count: 10,
            repair_count: 0,
            sure_signals: SureSignals::default(),
            cache_result: CacheResult::Miss,
            failure_class: None,
            bytes_per_vector: 0.0,
        })
    };

    let t1 = make_trace("trace-a");
    let t2 = make_trace("trace-b");

    let inputs = vec![
        TraceBatchInput {
            trace: &t1,
            corpus_gb: 1.0,
            total_vectors: 1_000_000,
            tier_label: "free",
            occurred_at_ms: 1_700_000_000_000,
            occurred_at_iso: "2026-05-22T00:00:00Z".into(),
            idempotency_bucket: Duration::from_secs(1),
        },
        TraceBatchInput {
            trace: &t2,
            corpus_gb: 1.0,
            total_vectors: 1_000_000,
            tier_label: "free",
            occurred_at_ms: 1_700_000_000_000,
            occurred_at_iso: "2026-05-22T00:00:00Z".into(),
            idempotency_bucket: Duration::from_secs(1),
        },
    ];

    let batch = build_batch(&inputs);
    assert_eq!(batch.count, 2);
    // Same shape → 1 distinct fingerprint.
    assert_eq!(batch.distinct_fingerprints, 1);
    assert_eq!(batch.records[0].fingerprint, batch.records[1].fingerprint);
    // Distinct trace_ids → 2 distinct idempotency keys.
    assert_ne!(
        batch.records[0].idempotency_key,
        batch.records[1].idempotency_key
    );
}

/// Cross-tenant batch: same trace_id under different tenants must
/// produce two distinct idempotency keys (digest mixes tenant_id in).
/// Same shape → same fingerprint. Both invariants the per-primitive
/// tests cover individually, asserted together at the integration
/// boundary.
#[test]
fn cross_tenant_traces_share_fingerprint_but_have_distinct_idempotency_keys() {
    let tier_a = TenantTierRecord::fail_safe("tenant-a");
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    let plan = build_for_search(&PlanBuilderInputs {
        predicates: &[],
        field_stats: &stats,
        policy: &policy,
        gls_samples: &[],
        dim: 768,
        recall_target: 0.9,
        collection_gb: 0.1,
        tier: &tier_a,
    });

    let make_trace = |tenant: &str| -> SearchPlanTrace {
        build_trace(TraceBuilderInputs {
            trace_id: "trace-X".into(),
            tenant_id: tenant.into(),
            collection_name: "kb".into(),
            plan: &plan,
            latency_ms: 12.3,
            index_stats: IndexStats::default(),
            candidate_count: 64,
            rerank_count: 10,
            repair_count: 0,
            sure_signals: SureSignals::default(),
            cache_result: CacheResult::Miss,
            failure_class: None,
            bytes_per_vector: 0.0,
        })
    };

    let t_a = make_trace("tenant-a");
    let t_b = make_trace("tenant-b");

    let inputs = vec![
        TraceBatchInput {
            trace: &t_a,
            corpus_gb: 1.0,
            total_vectors: 1_000_000,
            tier_label: "free",
            occurred_at_ms: 1_700_000_000_000,
            occurred_at_iso: "2026-05-22T00:00:00Z".into(),
            idempotency_bucket: Duration::from_secs(1),
        },
        TraceBatchInput {
            trace: &t_b,
            corpus_gb: 1.0,
            total_vectors: 1_000_000,
            tier_label: "free",
            occurred_at_ms: 1_700_000_000_000,
            occurred_at_iso: "2026-05-22T00:00:00Z".into(),
            idempotency_bucket: Duration::from_secs(1),
        },
    ];
    let batch = build_batch(&inputs);
    assert_eq!(batch.records[0].fingerprint, batch.records[1].fingerprint);
    assert_ne!(
        batch.records[0].idempotency_key,
        batch.records[1].idempotency_key
    );
    // The metering events carry distinct tenant_ids.
    assert_eq!(batch.records[0].metering["tenant_id"], "tenant-a");
    assert_eq!(batch.records[1].metering["tenant_id"], "tenant-b");
}

/// Plan boundary case: a non-trivial planner input (small dim + low
/// recall + reasonable collection) must still flow through every
/// stage without panicking. Catches accidental coupling between
/// `PlanOutput` and `TraceBuilder` when a non-default route is chosen.
#[test]
fn non_default_plan_choice_propagates_through_the_pipeline() {
    let tier = TenantTierRecord::fail_safe("tenant-a");
    let stats = FieldStatistics::default();
    let policy = PredicateSelectivityPolicy::default();
    // Direct construction of PlanInputs to force a specific band —
    // this is the same flow build_for_search uses, but at the boundary
    // we want to see the route choice survive when it isn't the
    // default.
    let plan = build_for_search(&PlanBuilderInputs {
        predicates: &[],
        field_stats: &stats,
        policy: &policy,
        gls_samples: &[],
        dim: 1536, // XLarge bucket
        recall_target: 0.97,
        collection_gb: 0.01,
        tier: &tier,
    });
    // XLarge dim + high recall on a small collection → Quantized route.
    assert_eq!(plan.index_route, IndexRoute::QuantizedGraphThenExact);

    let trace = build_trace(TraceBuilderInputs {
        trace_id: "trace-q".into(),
        tenant_id: "tenant-a".into(),
        collection_name: "kb".into(),
        plan: &plan,
        latency_ms: 18.0,
        index_stats: IndexStats::default(),
        candidate_count: 32,
        rerank_count: 5,
        repair_count: 0,
        sure_signals: SureSignals::default(),
        cache_result: CacheResult::Miss,
        failure_class: None,
        bytes_per_vector: 0.0,
    });
    assert_eq!(trace.index_route, IndexRoute::QuantizedGraphThenExact);

    // Metering carries the route label in snake_case.
    let metering = build_kru(&MeteringInputs {
        trace: &trace,
        tier_label: "free",
        corpus_gb: 0.01,
        total_vectors: 1_000,
        occurred_at: "2026-05-22T00:00:00Z".into(),
    });
    assert_eq!(
        metering.metadata["index_route"],
        "quantized_graph_then_exact"
    );

    // Fingerprint carries the quantized_route_taken flag.
    let shape = TraceShape::from_trace(&trace, 0.01);
    assert!(shape.quantized_route_taken);
    let fp = fingerprint_hex(&shape);
    assert_eq!(fp.len(), 16);
}

/// Sanity: PlanInputs construction outside the test scope to ensure
/// the import is exercised (compile-time enforcement).
#[test]
fn plan_inputs_struct_is_reachable_from_integration_scope() {
    let _pi = PlanInputs {
        selectivity: 0.5,
        gls_score: None,
        dim: 768,
        recall_target: 0.9,
        collection_gb: 1.0,
    };
}
