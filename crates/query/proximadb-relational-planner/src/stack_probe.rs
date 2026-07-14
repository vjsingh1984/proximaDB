// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0

//! Stack high-water probe — measure the real stack consumed by the plan-tree
//! recursions (TD-EXEC-2 Slice 1, observe-only).
//!
//! TD-EXEC-2 §1.b needs `frame_bytes[op_kind]` to turn the geometry vector into
//! a stack estimate, but the per-frame cost is a ~100× unknown (TD-EXEC-1 says
//! 100–200 KB/frame; a code probe suggested ~1 KB). This module measures it:
//! [`probe`] arms a thread-local low-water mark around a closure, and [`note`]
//! — called at the entry of each guarded recursion level (the planner's
//! `lower_to_physical`, the executor's `build_executor`, the native `walk`) —
//! samples [`stacker::remaining_stack`] into it. The difference between the
//! remaining stack at arm time and the observed minimum is the recursion's
//! stack high-water mark in bytes.
//!
//! Cost: when no probe is armed, [`note`] is a TLS read + branch (~ns) — the
//! same order as the `stacker::maybe_grow` check it sits next to. This is a
//! measurement primitive only; it makes no decision and changes no behavior.
//!
//! ## Accuracy under `maybe_grow`
//!
//! [`stacker::maybe_grow`] switches deep descents onto freshly-allocated
//! segments, and [`stacker::remaining_stack`] then reports headroom *within the
//! current segment*. Samples taken on a grown segment are not comparable to the
//! arm-time baseline, so past the first growth the reported high-water mark is
//! a **lower bound**, not an exact figure. In practice this is fine: the
//! observe-only trace flags such plans by their geometry (`max_depth`), and the
//! calibration tests run on a dedicated large-stack thread where no growth
//! occurs and the measurement is exact.

use std::cell::Cell;

thread_local! {
    /// Remaining-stack low-water mark of the armed probe on this thread;
    /// `None` when no probe is armed.
    static LOW_WATER: Cell<Option<usize>> = const { Cell::new(None) };
}

/// Sample the remaining stack into the armed probe's low-water mark.
///
/// Call at the entry of a guarded recursion level. No-op (a TLS read and a
/// branch) when no [`probe`] is armed on this thread or when the platform
/// cannot report the remaining stack.
#[inline]
pub fn note() {
    LOW_WATER.with(|lw| {
        if let Some(current_low) = lw.get()
            && let Some(remaining) = stacker::remaining_stack()
            && remaining < current_low
        {
            lw.set(Some(remaining));
        }
    });
}

/// Restores the previously-armed probe state on drop, so a panicking closure
/// cannot leave a stale probe armed on the thread.
struct Rearm(Option<usize>);

impl Drop for Rearm {
    fn drop(&mut self) {
        LOW_WATER.with(|lw| lw.set(self.0.take()));
    }
}

/// Run `f` with a stack probe armed and return `(f(), high_water_bytes)`.
///
/// `high_water_bytes` is the maximum stack depth [`note`] observed below the
/// arm point, i.e. the stack the guarded recursions inside `f` actually
/// consumed. `0` when the platform cannot report the remaining stack or when
/// nothing called [`note`].
///
/// Probes nest safely (the inner probe shadows, then restores, the outer), but
/// the outer probe does not see samples taken while an inner probe is armed.
pub fn probe<T>(f: impl FnOnce() -> T) -> (T, u64) {
    let start = stacker::remaining_stack();
    let rearm = Rearm(LOW_WATER.with(|lw| lw.replace(start)));
    let out = f();
    let low = LOW_WATER.with(|lw| lw.get());
    drop(rearm);
    let high_water = match (start, low) {
        (Some(start), Some(low)) => start.saturating_sub(low) as u64,
        _ => 0,
    };
    (out, high_water)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probe_without_note_reports_zero() {
        let (value, hwm) = probe(|| 42);
        assert_eq!(value, 42);
        assert_eq!(hwm, 0);
    }

    #[test]
    fn note_outside_probe_is_a_noop() {
        note(); // must not panic or arm anything
        let (_, hwm) = probe(|| ());
        assert_eq!(hwm, 0);
    }

    #[test]
    fn probe_measures_recursion_depth() {
        // A recursion that notes at each level and pins a real frame via a
        // volatile-ish buffer the optimizer cannot elide.
        fn descend(depth: usize) -> usize {
            note();
            let buf = [0u8; 1024];
            std::hint::black_box(&buf);
            if depth == 0 {
                buf.len()
            } else {
                std::hint::black_box(descend(depth - 1))
            }
        }
        let (_, shallow) = probe(|| descend(4));
        let (_, deep) = probe(|| descend(128));
        // Each frame holds a 1 KiB buffer, so 124 extra frames must consume
        // at least 124 KiB more stack (when the platform reports it at all).
        if shallow > 0 || deep > 0 {
            assert!(
                deep >= shallow + 124 * 1024,
                "deep recursion must show a higher stack high-water mark: \
                 shallow={shallow} deep={deep}"
            );
        }
    }

    #[test]
    fn nested_probe_restores_the_outer() {
        fn descend(depth: usize) -> usize {
            note();
            let buf = [0u8; 1024];
            std::hint::black_box(&buf);
            if depth == 0 {
                buf.len()
            } else {
                std::hint::black_box(descend(depth - 1))
            }
        }
        let ((_, inner_hwm), outer_hwm) = probe(|| {
            let inner = probe(|| descend(32));
            descend(64); // sampled by the OUTER probe after the inner restored
            inner
        });
        if inner_hwm > 0 {
            // The outer probe armed higher up the stack, so its high-water on
            // the same-depth descent is at least the inner's.
            assert!(outer_hwm >= inner_hwm);
        }
    }
}
