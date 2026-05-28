/*
 * Copyright 2025 ProximaDB
 *
 * Licensed under the Apache License, Version 2.0 (the "License");
 * you may not use this file except in compliance with the License.
 */

//! Per-request predicate-diagnostics bus (TD-064)
//!
//! Carries `PredicateShortfall` from `AxisManager`-deep search paths up to
//! the request handler that builds the `SearchPlanTrace`, without forcing
//! every intermediate service / storage-engine / proto type to declare a
//! `predicate_shortfall` field.
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
}
