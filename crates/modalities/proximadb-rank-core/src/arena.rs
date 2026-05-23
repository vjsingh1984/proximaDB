//! Per-query allocation arena for feature outputs.
//!
//! Backed by `bumpalo` for O(1) reset between docs (first phase) and
//! between batches (second phase). The hot per-doc path stays
//! allocation-free in steady state once the arena has grown enough to
//! absorb peak demand.
//!
//! See `roadmap/RANKING_FRAMEWORK_SPEC_2026_05_23.md` §4.1.3.

use bumpalo::Bump;
use std::cell::Cell;

/// Arena allocator threaded through the rank pipeline.
///
/// Owners are `RankProgram` instances. Each call to `rank()` may emit
/// tensor allocations that are valid until the next `reset()`.
pub struct FeatureArena {
    bump: Bump,
    high_water: Cell<usize>,
    reset_count: Cell<u64>,
}

impl FeatureArena {
    pub fn new() -> Self {
        Self {
            bump: Bump::new(),
            high_water: Cell::new(0),
            reset_count: Cell::new(0),
        }
    }

    /// Pre-sized constructor — useful in production where the typical
    /// peak per-query allocation is known.
    pub fn with_capacity(bytes: usize) -> Self {
        Self {
            bump: Bump::with_capacity(bytes),
            high_water: Cell::new(0),
            reset_count: Cell::new(0),
        }
    }

    /// Allocate a slice of `f32` initialised from `data`. Returned slice
    /// is valid until the next `reset()`.
    pub fn alloc_floats<'a>(&'a self, data: &[f32]) -> &'a mut [f32] {
        let slice = self.bump.alloc_slice_copy(data);
        self.bump_high_water();
        slice
    }

    /// Allocate an uninitialised slice of `f32` of `len` elements.
    pub fn alloc_floats_uninit(&self, len: usize) -> &mut [f32] {
        let slice = self.bump.alloc_slice_fill_default(len);
        self.bump_high_water();
        slice
    }

    /// Reset all allocations. O(1) — bumpalo just rewinds the bump pointer.
    pub fn reset(&mut self) {
        self.reset_count.set(self.reset_count.get() + 1);
        self.bump.reset();
    }

    /// Current peak allocated bytes across the arena's lifetime.
    /// Used by observability to size future arenas.
    pub fn high_water_bytes(&self) -> usize {
        self.high_water.get()
    }

    /// Total resets since construction. Useful for sanity checks in tests.
    pub fn reset_count(&self) -> u64 {
        self.reset_count.get()
    }

    fn bump_high_water(&self) {
        let used = self.bump.allocated_bytes();
        if used > self.high_water.get() {
            self.high_water.set(used);
        }
    }
}

impl Default for FeatureArena {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Instant;

    #[test]
    fn alloc_floats_returns_referenceable_slice() {
        let arena = FeatureArena::new();
        let s = arena.alloc_floats(&[1.0, 2.0, 3.0]);
        assert_eq!(s, &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn alloc_uninit_returns_zero_initialised() {
        let arena = FeatureArena::new();
        let s = arena.alloc_floats_uninit(8);
        assert_eq!(s.len(), 8);
        for v in s.iter() {
            assert_eq!(*v, 0.0);
        }
    }

    #[test]
    fn reset_increments_counter() {
        let mut arena = FeatureArena::new();
        assert_eq!(arena.reset_count(), 0);
        arena.reset();
        arena.reset();
        assert_eq!(arena.reset_count(), 2);
    }

    #[test]
    fn high_water_tracks_peak_usage() {
        let arena = FeatureArena::new();
        let _a = arena.alloc_floats(&[0.0; 64]); // 256 bytes
        let hw1 = arena.high_water_bytes();
        let _b = arena.alloc_floats(&[0.0; 128]); // +512 bytes
        let hw2 = arena.high_water_bytes();
        assert!(hw2 > hw1, "high water must rise after a larger alloc");
    }

    #[test]
    fn reset_is_constant_time_under_load() {
        // Sanity bench: 10k allocations of 256 bytes each, then 10k resets.
        // Expectation: well under 50ms wall-clock on any modern machine.
        // This is not a hard NFR gate (that lives in R-3) — it just guards
        // against an O(N) regression where reset would walk all allocations.
        let mut arena = FeatureArena::with_capacity(64 * 1024);
        for _ in 0..10_000 {
            let _ = arena.alloc_floats(&[1.0_f32; 64]);
        }
        let t0 = Instant::now();
        for _ in 0..10_000 {
            arena.reset();
            let _ = arena.alloc_floats(&[1.0_f32; 64]);
        }
        let elapsed = t0.elapsed();
        assert!(
            elapsed.as_millis() < 100,
            "10k reset+alloc cycles took {}ms — possible O(N) regression",
            elapsed.as_millis()
        );
    }

    #[test]
    fn arena_default_works() {
        let _ = FeatureArena::default();
    }
}
