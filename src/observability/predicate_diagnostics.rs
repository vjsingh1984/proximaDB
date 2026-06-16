/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Per-request search-diagnostics bus (TD-064, TD-075/F2)
//!
//! Carries `AxisManager`-deep search diagnostics up to the request handler
//! that builds the `SearchPlanTrace` / EXPLAIN, without forcing every
//! intermediate service / storage-engine / proto type to declare a field for
//! each signal. Today it carries two:
//!
//! * `PredicateShortfall` (TD-064) — predicate-aware search returned fewer
//!   than `k` results.
//! * a quantized-route **downgrade** flag (TD-075 / Phase 8 F2) — the index had
//!   quantized storage but the recall-probe gate forced exact search, so the
//!   degraded route can be disclosed in EXPLAIN.
//!
//! Concretely:
//!
//! 1. The REST/gRPC handler wraps the search call in
//!    [`scope`] before invoking downstream code.
//! 2. Whenever any layer detects a shortfall it calls
//!    [`record_shortfall`], which writes into the active task-local
//!    diagnostics struct.
//! 3. After the search call returns, the handler calls
//!    [`take_shortfall`] to retrieve the captured shortfall (if any) and
//!    passes it into `TraceBuilderInputs.predicate_shortfall`.
//!
//! ## Why task-local
//!
//! Tokio task-local state binds a value to the future being polled,
//! including across `.await` points. This means a single request's
//! AxisManager invocation can write into a context the handler set up,
//! even when there are arbitrarily many service / storage layers between
//! them — without those layers needing to know the diagnostics exist.
//!
//! ## Semantics on missing scope
//!
//! If `record_shortfall` is called outside an active `scope` (e.g., the
//! caller is in a test harness or a path that hasn't been wired through
//! yet), the call silently no-ops. This keeps direct `AxisManager` users
//! working — they still get the structured `PredicateShortfall` on the
//! returned tuple and the Prometheus counter still fires.
//!
//! ## First-writer-wins
//!
//! Nested search calls within the same task will only record the FIRST
//! shortfall. This is intentional: the first shortfall is the one most
//! directly tied to the user's request; later shortfalls in cascaded
//! lookups are noise. Operators who need finer detail can read the
//! Prometheus counters instead, which count every event.

use std::sync::Mutex;

use crate::observability::search_plan_trace::PredicateShortfall;

tokio::task_local! {
    /// Active diagnostics bus for the current task. Bound by [`scope`].
    static PREDICATE_DIAGNOSTICS: PredicateDiagnostics;
}

/// Per-request collector for predicate-aware diagnostics.
///
/// Held inside a [`tokio::task_local!`] so any depth of the search call
/// stack can append findings without explicit threading.
#[derive(Debug, Default)]
pub struct PredicateDiagnostics {
    shortfall: Mutex<Option<PredicateShortfall>>,
    /// Set when a search downgraded from the quantized route to exact because
    /// the recall-probe gate was closed (TD-075 / F2). Surfaced in EXPLAIN.
    quantized_downgraded: Mutex<bool>,
    /// Set when a search served Stage-1-only (Hamming, no fp32 rerank) because
    /// the IVF index was still cold-loaded (ADR-023 T-E `ColdBinaryOnly`).
    cold_stage1_only: Mutex<bool>,
    /// TurboQuant EXPLAIN hint payload (Phase J — Quantization Trait
    /// Convergence Plan). Carries the `TurboQuantExplainHints` JSON value
    /// recorded by `score_turboquant`. Read at the request boundary into
    /// `SearchPlanHints.turboquant`, then propagated to `VectorHints` via
    /// `VectorHints::from(&SearchPlanHints)` for the wire-facing EXPLAIN
    /// payload. See `src/index/turboquant_bridge.rs` for the 9-field
    /// schema.
    turboquant_hints: Mutex<Option<serde_json::Value>>,
    /// TD-040: number of SST blocks skipped by per-block vector-bounds (L2
    /// lower-bound) pruning before their data blocks were read. Accumulated
    /// (summed) across any searches in the request — a count, not a one-shot
    /// flag — then surfaced in EXPLAIN via the `vector_bounds_pruned` hint.
    vector_bounds_pruned_blocks: Mutex<u64>,
}

impl PredicateDiagnostics {
    /// Create an empty diagnostics container.
    pub fn new() -> Self {
        Self::default()
    }

    /// Record a shortfall. First-writer-wins — subsequent records are
    /// ignored. Returns `true` if this call was the first writer.
    pub fn record_shortfall(&self, sf: PredicateShortfall) -> bool {
        let mut g = self.shortfall.lock().unwrap_or_else(|p| p.into_inner());
        if g.is_none() {
            *g = Some(sf);
            true
        } else {
            false
        }
    }

    /// Atomically take the captured shortfall, leaving the container empty.
    pub fn take_shortfall(&self) -> Option<PredicateShortfall> {
        self.shortfall
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    }

    /// Mark that the quantized route was downgraded to exact (gate closed).
    /// Idempotent — repeated calls within a request keep the flag set.
    pub fn record_quantized_downgrade(&self) {
        *self
            .quantized_downgraded
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = true;
    }

    /// Atomically take the downgrade flag, leaving the container cleared.
    pub fn take_quantized_downgrade(&self) -> bool {
        std::mem::take(
            &mut *self
                .quantized_downgraded
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        )
    }

    /// Mark that the search served Stage-1-only from a cold-loaded index.
    pub fn record_cold_stage1_only(&self) {
        *self
            .cold_stage1_only
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = true;
    }

    /// Atomically take the cold-Stage-1-only flag, leaving the container cleared.
    pub fn take_cold_stage1_only(&self) -> bool {
        std::mem::take(
            &mut *self
                .cold_stage1_only
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        )
    }

    /// Record TurboQuant EXPLAIN hints (Phase J/K — Quantization Trait
    /// Convergence Plan). Last-writer-wins because a single request may
    /// traverse multiple search stages with TurboQuant scoring, and the
    /// most recent hints carry the actionable kernel-side info
    /// (`blocks_skipped_by_mask`, `current_epoch`, etc.).
    ///
    /// Why last-writer (not first-writer like `shortfall`)? Predicate
    /// shortfalls are one-shot events tied directly to user intent; a
    /// later shortfall in a cascaded lookup is noise. TurboQuant hints
    /// are a per-stage tracing report — the latest is the one a
    /// post-mortem actually wants.
    pub fn record_turboquant_hints(&self, hints: serde_json::Value) {
        *self
            .turboquant_hints
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(hints);
    }

    /// Atomically take the TurboQuant hints, leaving the container empty.
    pub fn take_turboquant_hints(&self) -> Option<serde_json::Value> {
        self.turboquant_hints
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take()
    }

    /// Add to the running count of SST blocks skipped by vector-bounds pruning
    /// (TD-040). Accumulates because one request may issue multiple searches.
    pub fn record_vector_bounds_pruned(&self, blocks: u64) {
        let mut g = self
            .vector_bounds_pruned_blocks
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        *g = g.saturating_add(blocks);
    }

    /// Atomically take the vector-bounds-pruned block count, resetting to 0.
    pub fn take_vector_bounds_pruned(&self) -> u64 {
        std::mem::take(
            &mut *self
                .vector_bounds_pruned_blocks
                .lock()
                .unwrap_or_else(|p| p.into_inner()),
        )
    }
}

/// Bind a fresh diagnostics container to `future` and await it.
///
/// Handlers wrap the search call in this so downstream `record_shortfall`
/// calls have somewhere to write to. After the future returns, the
/// handler calls [`take_shortfall`] to read the captured value.
pub async fn scope<F: std::future::Future>(future: F) -> F::Output {
    PREDICATE_DIAGNOSTICS
        .scope(PredicateDiagnostics::new(), future)
        .await
}

/// Record a shortfall into the active diagnostics container.
///
/// Silently no-ops when there is no active [`scope`] (e.g., the caller
/// is outside the request path). The Prometheus counter is the
/// operator-visible signal in that case; this fn is for the
/// gateway/EXPLAIN path.
pub fn record_shortfall(sf: PredicateShortfall) {
    let _ = PREDICATE_DIAGNOSTICS.try_with(|d| {
        d.record_shortfall(sf);
    });
}

/// Take and return the captured shortfall, if any. Returns `None` when
/// there's no active scope OR no shortfall was recorded.
pub fn take_shortfall() -> Option<PredicateShortfall> {
    PREDICATE_DIAGNOSTICS
        .try_with(|d| d.take_shortfall())
        .unwrap_or(None)
}

/// Record a quantized-route downgrade (gate closed → exact) into the active
/// diagnostics container. Silently no-ops outside an active [`scope`].
pub fn record_quantized_downgrade() {
    let _ = PREDICATE_DIAGNOSTICS.try_with(|d| {
        d.record_quantized_downgrade();
    });
}

/// Take the quantized-route downgrade flag. Returns `false` when there's no
/// active scope OR no downgrade was recorded. Must be called inside the
/// [`scope`] that wrapped the search (the task-local binding ends when the
/// scoped future completes).
pub fn take_quantized_downgrade() -> bool {
    PREDICATE_DIAGNOSTICS
        .try_with(|d| d.take_quantized_downgrade())
        .unwrap_or(false)
}

/// Record that the search served Stage-1-only from a cold-loaded IVF index
/// (ADR-023 T-E). Silently no-ops outside an active [`scope`].
pub fn record_cold_stage1_only() {
    let _ = PREDICATE_DIAGNOSTICS.try_with(|d| {
        d.record_cold_stage1_only();
    });
}

/// Take the cold-Stage-1-only flag. Returns `false` outside an active [`scope`]
/// or when it was not recorded. Must be called inside the [`scope`].
pub fn take_cold_stage1_only() -> bool {
    PREDICATE_DIAGNOSTICS
        .try_with(|d| d.take_cold_stage1_only())
        .unwrap_or(false)
}

/// Record TurboQuant EXPLAIN hints into the active diagnostics container
/// (Phase J/K — Quantization Trait Convergence Plan). Silently no-ops
/// outside an active [`scope`] — the structured tracing event emitted
/// by `score_turboquant` remains the operator-visible signal in that
/// case.
pub fn record_turboquant_hints(hints: serde_json::Value) {
    let _ = PREDICATE_DIAGNOSTICS.try_with(|d| {
        d.record_turboquant_hints(hints);
    });
}

/// Take and return the captured TurboQuant hints, if any. Returns `None`
/// when there's no active scope OR no hints were recorded. Must be
/// called inside the [`scope`] that wrapped the search.
pub fn take_turboquant_hints() -> Option<serde_json::Value> {
    PREDICATE_DIAGNOSTICS
        .try_with(|d| d.take_turboquant_hints())
        .unwrap_or(None)
}

/// Add `blocks` to the active container's vector-bounds-pruned count (TD-040).
/// Silently no-ops outside an active [`scope`] — the `tracing::debug!` emitted
/// by the prune path remains the operator-visible signal in that case.
pub fn record_vector_bounds_pruned(blocks: u64) {
    if blocks == 0 {
        return;
    }
    let _ = PREDICATE_DIAGNOSTICS.try_with(|d| {
        d.record_vector_bounds_pruned(blocks);
    });
}

/// Take the accumulated vector-bounds-pruned block count. Returns 0 when there
/// is no active scope OR nothing was pruned. Must be called inside the [`scope`]
/// that wrapped the search.
pub fn take_vector_bounds_pruned() -> u64 {
    PREDICATE_DIAGNOSTICS
        .try_with(|d| d.take_vector_bounds_pruned())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_shortfall(returned: u32) -> PredicateShortfall {
        PredicateShortfall {
            requested_k: 10,
            returned_k: returned,
            oversample_pool: 20,
            ann_filtering_mode: "inline".into(),
        }
    }

    #[tokio::test]
    async fn record_outside_scope_is_silent_noop() {
        // No `scope` wrapping — this must not panic and take must yield None.
        record_shortfall(sample_shortfall(3));
        assert!(take_shortfall().is_none());
    }

    #[tokio::test]
    async fn record_inside_scope_is_visible_to_take() {
        let captured = scope(async {
            record_shortfall(sample_shortfall(3));
            take_shortfall()
        })
        .await;
        assert!(captured.is_some());
        let sf = captured.unwrap();
        assert_eq!(sf.returned_k, 3);
        assert_eq!(sf.requested_k, 10);
        assert_eq!(sf.ann_filtering_mode, "inline");
    }

    #[tokio::test]
    async fn first_writer_wins_across_records() {
        let captured = scope(async {
            record_shortfall(sample_shortfall(3));
            record_shortfall(sample_shortfall(5));
            take_shortfall()
        })
        .await;
        let sf = captured.expect("first record must survive");
        assert_eq!(sf.returned_k, 3, "first record's value must be preserved");
    }

    #[tokio::test]
    async fn take_clears_so_subsequent_take_returns_none() {
        let (first, second) = scope(async {
            record_shortfall(sample_shortfall(3));
            (take_shortfall(), take_shortfall())
        })
        .await;
        assert!(first.is_some());
        assert!(second.is_none());
    }

    #[tokio::test]
    async fn quantized_downgrade_records_and_takes_inside_scope() {
        let taken = scope(async {
            // Not yet recorded.
            assert!(!take_quantized_downgrade());
            record_quantized_downgrade();
            take_quantized_downgrade()
        })
        .await;
        assert!(taken, "downgrade recorded in-scope must be taken as true");
    }

    #[tokio::test]
    async fn quantized_downgrade_outside_scope_is_false() {
        // No scope wrapping — record no-ops, take yields false (never a panic).
        record_quantized_downgrade();
        assert!(!take_quantized_downgrade());
    }

    #[tokio::test]
    async fn cold_stage1_only_records_and_takes_inside_scope() {
        let taken = scope(async {
            assert!(!take_cold_stage1_only());
            record_cold_stage1_only();
            take_cold_stage1_only()
        })
        .await;
        assert!(taken, "cold-stage1 recorded in-scope must be taken as true");
        // Outside any scope: record no-ops, take is false.
        record_cold_stage1_only();
        assert!(!take_cold_stage1_only());
    }

    #[tokio::test]
    async fn quantized_downgrade_take_clears() {
        let (first, second) = scope(async {
            record_quantized_downgrade();
            (take_quantized_downgrade(), take_quantized_downgrade())
        })
        .await;
        assert!(first);
        assert!(!second, "take must clear the flag");
    }

    #[tokio::test]
    async fn scopes_are_isolated_between_independent_tasks() {
        // Outer scope: record sample(3). Inner scope: record sample(7),
        // take inside inner — must see 7, not 3.
        let outer = scope(async {
            record_shortfall(sample_shortfall(3));

            let inner_value = scope(async {
                record_shortfall(sample_shortfall(7));
                take_shortfall()
            })
            .await;

            (inner_value, take_shortfall())
        })
        .await;

        let (inner, outer_remaining) = outer;
        assert_eq!(inner.expect("inner had a record").returned_k, 7);
        assert_eq!(
            outer_remaining.expect("outer record preserved").returned_k,
            3,
            "inner scope must not leak into outer scope"
        );
    }

    #[tokio::test]
    async fn vector_bounds_pruned_accumulates_and_take_clears() {
        let (total, second) = scope(async {
            assert_eq!(take_vector_bounds_pruned(), 0);
            record_vector_bounds_pruned(3);
            record_vector_bounds_pruned(5); // accumulates (count, not flag)
            record_vector_bounds_pruned(0); // no-op
            (take_vector_bounds_pruned(), take_vector_bounds_pruned())
        })
        .await;
        assert_eq!(total, 8, "counts accumulate across records");
        assert_eq!(second, 0, "take clears the counter");
    }

    #[tokio::test]
    async fn vector_bounds_pruned_outside_scope_is_zero() {
        // No scope wrapping — record no-ops, take yields 0 (never a panic).
        record_vector_bounds_pruned(7);
        assert_eq!(take_vector_bounds_pruned(), 0);
    }

    // ------------------------------------------------------------------
    // TurboQuant hints (Phase J/K)
    // ------------------------------------------------------------------

    fn sample_turboquant_hints(blocks_skipped: u64) -> serde_json::Value {
        serde_json::json!({
            "quantization": "turboquant_4bit",
            "calibration_mode": "tq_plus",
            "rotation_seed": "0xdeadbeef",
            "encoded_epoch": 1,
            "mask_pushed_to_kernel": true,
            "kernel_arch": "scalar",
            "blocks_skipped_by_mask": blocks_skipped,
            "length_renorm_applied": true,
        })
    }

    #[tokio::test]
    async fn turboquant_record_outside_scope_is_silent_noop() {
        record_turboquant_hints(sample_turboquant_hints(0));
        assert!(take_turboquant_hints().is_none());
    }

    #[tokio::test]
    async fn turboquant_record_inside_scope_is_visible_to_take() {
        let captured = scope(async {
            record_turboquant_hints(sample_turboquant_hints(42));
            take_turboquant_hints()
        })
        .await;
        let v = captured.expect("recorded value retrieved");
        assert_eq!(v["blocks_skipped_by_mask"], 42);
        assert_eq!(v["quantization"], "turboquant_4bit");
    }

    #[tokio::test]
    async fn turboquant_last_writer_wins_across_records() {
        // Unlike shortfall (first-writer-wins), TurboQuant hints are a
        // per-stage tracing report — the latest is the actionable one
        // (`blocks_skipped_by_mask` accumulates across stages).
        let captured = scope(async {
            record_turboquant_hints(sample_turboquant_hints(1));
            record_turboquant_hints(sample_turboquant_hints(2));
            record_turboquant_hints(sample_turboquant_hints(3));
            take_turboquant_hints()
        })
        .await;
        let v = captured.expect("at least one writer");
        assert_eq!(
            v["blocks_skipped_by_mask"], 3,
            "last writer must win for TurboQuant hints",
        );
    }

    #[tokio::test]
    async fn turboquant_take_clears_the_slot() {
        // Calling take twice yields the value then None.
        let (first, second) = scope(async {
            record_turboquant_hints(sample_turboquant_hints(7));
            let a = take_turboquant_hints();
            let b = take_turboquant_hints();
            (a, b)
        })
        .await;
        assert!(first.is_some());
        assert!(second.is_none(), "take must clear the slot");
    }

    #[tokio::test]
    async fn turboquant_scope_isolation_inner_does_not_leak_to_outer() {
        // Nested scope must isolate — the outer's hint survives, the
        // inner's hint is contained.
        let outer = scope(async {
            record_turboquant_hints(sample_turboquant_hints(11));
            let inner_value = scope(async {
                record_turboquant_hints(sample_turboquant_hints(22));
                take_turboquant_hints()
            })
            .await;
            (inner_value, take_turboquant_hints())
        })
        .await;
        let (inner, outer_remaining) = outer;
        assert_eq!(
            inner.expect("inner record").get("blocks_skipped_by_mask"),
            Some(&serde_json::Value::from(22))
        );
        assert_eq!(
            outer_remaining
                .expect("outer record preserved")
                .get("blocks_skipped_by_mask"),
            Some(&serde_json::Value::from(11)),
            "inner scope must not leak into outer scope",
        );
    }
}
