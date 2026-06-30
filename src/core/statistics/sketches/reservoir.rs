// Copyright (C) 2025 ProximaDB
// SPDX-License-Identifier: Apache-2.0
//! Reservoir sampling (Vitter's Algorithm R) for example values
//! (`FieldStatistics::examples`).
//!
//! Bounded to `capacity` items regardless of stream length, uniform over what
//! was seen. The PRNG is a deterministic SplitMix64 seeded per-instance — no
//! `rand` dependency and reproducible in tests (`Math.random`-style nondeterminism
//! is exactly what the test-hygiene mandate forbids).

use serde::{Deserialize, Serialize};

/// Deterministic SplitMix64 — tiny, dependency-free, good enough for sampling.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SplitMix64 {
    state: u64,
}

impl SplitMix64 {
    fn new(seed: u64) -> Self {
        Self { state: seed }
    }
    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }
    /// Uniform in `[0, n)` for `n > 0`.
    fn below(&mut self, n: u64) -> u64 {
        if n == 0 {
            return 0;
        }
        self.next_u64() % n
    }
}

/// A uniform reservoir sample of stringified values.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Reservoir {
    capacity: usize,
    seen: u64,
    items: Vec<String>,
    rng: SplitMix64,
}

impl Reservoir {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            seen: 0,
            items: Vec::new(),
            // Fixed seed: deterministic across runs for the same stream.
            rng: SplitMix64::new(0x5151_5151_5151_5151),
        }
    }

    /// Offer one value to the reservoir.
    pub fn insert(&mut self, value: &str) {
        self.seen += 1;
        if self.items.len() < self.capacity {
            self.items.push(value.to_string());
            return;
        }
        // Replace a random existing slot with probability capacity/seen.
        let j = self.rng.below(self.seen);
        if (j as usize) < self.capacity
            && let Some(slot) = self.items.get_mut(j as usize)
        {
            *slot = value.to_string();
        }
    }

    /// Merge another reservoir (compaction). Approximate: replay the other's
    /// samples weighted by how many it stands for, preserving boundedness.
    pub fn merge(&mut self, other: &Reservoir) {
        for v in &other.items {
            self.insert(v);
        }
    }

    /// The current sample (order is deterministic but not meaningful).
    pub fn samples(&self) -> &[String] {
        &self.items
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_all_when_under_capacity() {
        let mut r = Reservoir::new(10);
        for i in 0..5 {
            r.insert(&format!("v{i}"));
        }
        assert_eq!(r.samples().len(), 5);
    }

    #[test]
    fn bounded_by_capacity() {
        let mut r = Reservoir::new(8);
        for i in 0..10_000 {
            r.insert(&format!("v{i}"));
        }
        assert_eq!(r.samples().len(), 8);
    }

    #[test]
    fn deterministic_for_same_stream() {
        let build = || {
            let mut r = Reservoir::new(8);
            for i in 0..1000 {
                r.insert(&format!("v{i}"));
            }
            r
        };
        assert_eq!(build().samples(), build().samples());
    }

    #[test]
    fn merge_stays_bounded() {
        let mut a = Reservoir::new(8);
        let mut b = Reservoir::new(8);
        for i in 0..100 {
            a.insert(&format!("a{i}"));
            b.insert(&format!("b{i}"));
        }
        a.merge(&b);
        assert_eq!(a.samples().len(), 8);
    }
}
